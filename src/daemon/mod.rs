//! The receiver daemon (Mac side). Backgrounds itself, owns the TCP listener, the SSH
//! tunnel monitors, the control socket, and the output FIFO. Uses the `smol` async
//! runtime; SSH children are waited on via `smol::process` (never a blocking `wait()`).
//!
//! See `specs/daemon.md` and `specs/ssh-connection.md`.

use crate::error::{Error, Result};
use crate::frame::{self, LEN_PREFIX};
use crate::{paths, pidfile, protocol, state};
use nix::sys::signal::{SigSet, Signal};
use nix::unistd::Pid;
use smol::channel::{self, Sender};
use smol::io::{AsyncReadExt, AsyncWriteExt};
use smol::net::unix::UnixListener;
use smol::{Async, Executor, Timer};
use std::net::TcpListener as StdTcpListener;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const HEALTHY_THRESHOLD: Duration = Duration::from_secs(10);

/// Shared registry of SSH child PIDs (one slot per host, `None` while no child is live).
type ChildPids = Arc<Mutex<Vec<Option<i32>>>>;

/// Record (or clear, with `None`) the live SSH child PID for host `idx`.
fn set_child_pid(pids: &ChildPids, idx: usize, val: Option<i32>) {
    if let Ok(mut g) = pids.lock()
        && let Some(slot) = g.get_mut(idx)
    {
        *slot = val;
    }
}

/// Validate no live daemon, prepare the data dir/FIFO/socket, daemonize, then run the
/// async event loop: TCP accept + forward to FIFO, one SSH monitor per host, control
/// server, and signal handling (SIGTERM kills all SSH children and cleans up).
pub fn run_start(port: u16, log: Option<PathBuf>, hosts: Vec<String>) -> Result<()> {
    // --- 1. Refuse to start if a live daemon already exists ---
    let pid_path = paths::pid_file()?;
    if let Some(pid) = pidfile::read() {
        if pidfile::is_alive(pid) {
            return Err(Error::msg(format!("shpi is already running (pid {pid})")));
        }
        // Stale pid file — remove it so daemonize can write a fresh one.
        let _ = std::fs::remove_file(&pid_path);
    }

    // --- 2. Ensure data dir and FIFO ---
    paths::ensure_data_dir()?;
    let fifo = paths::fifo_path()?;
    if !fifo.exists() {
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600))
            .map_err(|e| Error::msg(format!("mkfifo: {e}")))?;
    }

    // --- 3. Bind the TCP listener before daemonizing so errors surface on the terminal ---
    let std_listener = StdTcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            Error::msg(format!("port {port} is already in use by another process"))
        } else {
            Error::from(e)
        }
    })?;

    // --- 4. Daemonize ---
    let log_path = match log {
        Some(p) => p,
        None => paths::default_log()?,
    };
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_file2 = log_file.try_clone()?;

    daemonize::Daemonize::new()
        .pid_file(&pid_path)
        .stdout(log_file)
        .stderr(log_file2)
        .start()
        .map_err(|e| Error::msg(format!("daemonize: {e}")))?;

    // --- 5. (Daemonized child) Block SIGTERM/SIGINT before starting any async threads ---
    let mut sigset = SigSet::empty();
    sigset.add(Signal::SIGTERM);
    sigset.add(Signal::SIGINT);
    sigset
        .thread_block()
        .map_err(|e| Error::msg(format!("sigprocmask: {e}")))?;

    // --- 6. Control socket: remove stale, then bind ---
    let sock_path = paths::control_sock()?;
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
    }
    // Bind synchronously so we own the path before spawning anything.
    let unix_listener = UnixListener::bind(&sock_path)
        .map_err(|e| Error::msg(format!("control socket bind: {e}")))?;

    // --- 7. Shared state ---
    let shared = state::SharedState::new(&hosts);

    // --- 8. Child PID registry for signal-driven cleanup ---
    let child_pids: ChildPids = Arc::new(Mutex::new(vec![None; hosts.len()]));

    // Spawn the signal-handler thread *after* the PID registry and paths are ready.
    {
        let child_pids = child_pids.clone();
        let pid_path = pid_path.clone();
        let sock_path = sock_path.clone();
        std::thread::spawn(move || {
            // Wait for SIGTERM or SIGINT.
            let _sig = sigset.wait().ok();

            // Kill every SSH child.
            if let Ok(pids) = child_pids.lock() {
                for &maybe_pid in pids.iter() {
                    if let Some(pid) = maybe_pid {
                        let _ = nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
                    }
                }
            }

            // Cleanup.
            let _ = std::fs::remove_file(&pid_path);
            let _ = std::fs::remove_file(&sock_path);
            std::process::exit(0);
        });
    }

    // --- Build and run the smol executor ---
    let ex = Arc::new(Executor::new());

    // FIFO writer channel: a single task drains the channel and writes to the FIFO.
    let (fifo_tx, fifo_rx) = channel::unbounded::<Vec<u8>>();

    // Spawn FIFO writer task.
    {
        let fifo = fifo.clone();
        ex.spawn(async move {
            // Open O_RDWR|O_NONBLOCK: holding both ends prevents open(2) from blocking
            // and prevents ENXIO on writes when no external reader is attached.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .expect("open fifo");

            while let Ok(frame) = fifo_rx.recv().await {
                // Single non-blocking write — POSIX guarantees atomicity up to PIPE_BUF
                // bytes when a frame fits. If the pipe is full (no reader draining it)
                // the write returns EAGAIN; we drop the frame rather than buffering.
                use std::io::Write;
                match (&file).write(&frame) {
                    Ok(_) => {}
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.raw_os_error() == Some(libc::EAGAIN) =>
                    {
                        // Pipe full or no consumer — drop silently per spec.
                    }
                    Err(e) => {
                        eprintln!("shpi: fifo write error: {e}");
                    }
                }
            }
        })
        .detach();
    }

    // Spawn TCP accept loop.
    {
        let ex2 = ex.clone();
        let fifo_tx2 = fifo_tx.clone();
        ex.spawn(async move {
            std_listener.set_nonblocking(true).expect("set_nonblocking");
            let listener = Async::new(std_listener).expect("async tcp listener");
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let tx = fifo_tx2.clone();
                        ex2.spawn(handle_connection(stream, tx)).detach();
                    }
                    Err(e) => {
                        eprintln!("shpi: accept error: {e}");
                    }
                }
            }
        })
        .detach();
    }

    // Spawn SSH monitor tasks — one per host.
    for (idx, addr) in hosts.iter().enumerate() {
        let addr = addr.clone();
        let shared = shared.clone();
        let child_pids = child_pids.clone();
        ex.spawn(run_tunnel_monitor(idx, addr, port, shared, child_pids))
            .detach();
    }

    // Spawn control socket server.
    {
        let ex2 = ex.clone();
        ex.spawn(async move {
            loop {
                match unix_listener.accept().await {
                    Ok((stream, _)) => {
                        let s = shared.clone();
                        ex2.spawn(handle_control(stream, s)).detach();
                    }
                    Err(e) => {
                        eprintln!("shpi: control accept error: {e}");
                    }
                }
            }
        })
        .detach();
    }

    // Drive all tasks forever; the signal thread exits the process.
    smol::block_on(ex.run(std::future::pending::<()>()));

    #[allow(unreachable_code)]
    Ok(())
}

/// Read exactly one frame from a TCP connection and forward it to the FIFO channel.
async fn handle_connection(stream: Async<std::net::TcpStream>, fifo_tx: Sender<Vec<u8>>) {
    let mut s = stream;

    // Read the 4-byte big-endian frame_len.
    let mut len_buf = [0u8; LEN_PREFIX];
    if s.read_exact(&mut len_buf).await.is_err() {
        return;
    }
    let frame_len = u32::from_be_bytes(len_buf) as usize;
    if frame_len > frame::MAX_FRAME_LEN {
        return; // reject an implausible length rather than allocating for it
    }

    // Allocate the full frame once (prefix + payload) and read the payload straight
    // into place, so the FIFO writer receives the bytes verbatim.
    let mut buf = vec![0u8; LEN_PREFIX + frame_len];
    buf[..LEN_PREFIX].copy_from_slice(&len_buf);
    if s.read_exact(&mut buf[LEN_PREFIX..]).await.is_err() {
        return;
    }
    let _ = fifo_tx.try_send(buf);
}

/// Serve one control-socket connection: snapshot state, encode, write, close.
async fn handle_control(mut stream: smol::net::unix::UnixStream, shared: state::SharedState) {
    let snapshot = shared.snapshot();
    let reply = protocol::encode_report(&snapshot);
    let _ = stream.write_all(reply.as_bytes()).await;
}

/// Mark host `idx` as reconnecting, sleep for the current `backoff`, then double it
/// (capped at `MAX_BACKOFF`). Shared by the spawn-failure and clean-exit paths.
async fn schedule_reconnect(
    idx: usize,
    shared: &state::SharedState,
    backoff: &mut Duration,
    attempt: &mut u32,
) {
    *attempt = attempt.saturating_add(1);
    let next_retry = SystemTime::now() + *backoff;
    shared.set(
        idx,
        state::TunnelStatus::Reconnecting {
            attempt: *attempt,
            next_retry,
        },
    );
    Timer::after(*backoff).await;
    *backoff = (*backoff * 2).min(MAX_BACKOFF);
}

/// Supervise one SSH reverse-tunnel, reconnecting with exponential backoff.
async fn run_tunnel_monitor(
    idx: usize,
    addr: String,
    port: u16,
    shared: state::SharedState,
    child_pids: ChildPids,
) {
    let mut backoff = INITIAL_BACKOFF;
    let mut attempt: u32 = 0;

    loop {
        // Spawn the SSH child.
        let mut cmd = smol::process::Command::new("ssh");
        cmd.args([
            "-N",
            &format!("-R{port}:localhost:{port}"),
            "-oServerAliveInterval=30",
            "-oServerAliveCountMax=3",
            "-oExitOnForwardFailure=yes",
            &addr,
        ]);
        cmd.stdin(smol::process::Stdio::null());
        cmd.stdout(smol::process::Stdio::null());
        cmd.stderr(smol::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                // ssh failed to launch at all (e.g. missing binary). Log it — the
                // status state only tracks the backoff — then retry.
                eprintln!("shpi: failed to spawn ssh for {addr}: {e}");
                schedule_reconnect(idx, &shared, &mut backoff, &mut attempt).await;
                continue;
            }
        };

        // Record the child PID.
        let pid = child.id() as i32;
        set_child_pid(&child_pids, idx, Some(pid));

        let connection_start = Instant::now();
        shared.set(
            idx,
            state::TunnelStatus::Connected {
                since: SystemTime::now(),
            },
        );

        // Await child exit without blocking the executor.
        let _status = child.status().await;
        set_child_pid(&child_pids, idx, None);

        // A connection that stayed up long enough resets the backoff.
        if connection_start.elapsed() > HEALTHY_THRESHOLD {
            backoff = INITIAL_BACKOFF;
        }

        schedule_reconnect(idx, &shared, &mut backoff, &mut attempt).await;
    }
}
