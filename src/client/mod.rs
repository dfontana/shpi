//! Client-side commands run on the user's terminal (not the daemon): `send` (remote
//! Linux side), and `watch` / `status` / `stop` (Mac side). All are synchronous std
//! code — none of them use the async runtime.
//!
//! Implement each function below against the shared foundation:
//! - [`crate::frame`] — frame encode/decode (`encode_frame`, `read_frame`).
//! - [`crate::protocol`] — `decode_report` for the control-socket status reply.
//! - [`crate::state`] — `HostReport` / `TunnelStatus` types.
//! - [`crate::paths`] — file locations (`pid_file`, `fifo_path`, `control_sock`).

use crate::error::{Error, Result};
use crate::frame;
use crate::paths;
use crate::pidfile;
use crate::protocol;
use crate::state::TunnelStatus;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::time::{Duration, SystemTime};

// ─── send ────────────────────────────────────────────────────────────────────

/// `shpi send`: build one frame (host = `name` or the system hostname, body = `message`),
/// open a TCP connection to `127.0.0.1:port`, write the single frame, close. See
/// `specs/requests.md`.
pub fn send(port: u16, name: Option<String>, message: String) -> Result<()> {
    let host = match name {
        Some(n) => n,
        None => {
            let raw = nix::unistd::gethostname()
                .map_err(|e| Error::msg(format!("could not get hostname: {e}")))?;
            raw.to_string_lossy().into_owned()
        }
    };

    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect(&addr)
        .map_err(|_| Error::msg(format!("could not connect to {addr} (is a tunnel up?)")))?;

    let frame_bytes = frame::encode_frame(&host, message.as_bytes());
    stream.write_all(&frame_bytes)?;
    // Dropping `stream` closes the connection — exactly one frame per connection.
    Ok(())
}

// ─── watch ───────────────────────────────────────────────────────────────────

/// `shpi watch`: open the FIFO for reading and stream frames as they arrive, flushing
/// after each. Default output: `host \t body` line. With `osc99`: emit a
/// kitty desktop notification with `<host>` as title and `<body>` as body
/// (two OSC-99 chunks). Blocks until interrupted. See `specs/cli.md`.
pub fn watch(osc99: bool) -> Result<()> {
    let fifo = paths::fifo_path()?;
    if !fifo.exists() {
        return Err(Error::msg(
            "FIFO not found — is the shpi daemon running? (start it with `shpi start`)",
        ));
    }

    let mut file = std::fs::File::open(&fifo)?;
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    loop {
        match frame::read_frame(&mut file) {
            Ok(Some(fr)) => {
                let body = String::from_utf8_lossy(&fr.body);
                let write_result = if osc99 {
                    // OSC-99 (kitty desktop notifications): the single payload
                    // field is the title by default. Title and body are sent as
                    // two chunks sharing an identifier; `d=0` keeps the
                    // notification open, `d=1` closes it.
                    //   ESC]99;i=1:d=0:p=title;<host>ESC\
                    //   ESC]99;i=1:d=1:p=body;<body>ESC\
                    write!(
                        out,
                        "\x1b]99;i=1:d=0:p=title;{}\x1b\\\x1b]99;i=1:d=1:p=body;{}\x1b\\",
                        fr.host, body
                    )
                } else {
                    writeln!(out, "{}\t{}", fr.host, body)
                };
                if stop_on_broken_pipe(write_result)? || stop_on_broken_pipe(out.flush())? {
                    return Ok(());
                }
            }
            Ok(None) => {
                // Writer/daemon closed the FIFO — exit cleanly.
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Treat a broken pipe (the reader went away) as a clean stop signal: `Ok(true)` means
/// stop, `Ok(false)` means continue. Any other write error propagates.
fn stop_on_broken_pipe(r: io::Result<()>) -> Result<bool> {
    match r {
        Ok(()) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(true),
        Err(e) => Err(e.into()),
    }
}

// ─── status ──────────────────────────────────────────────────────────────────

/// `shpi status`: query the daemon's control socket, decode the reply, and print a
/// human-readable per-host summary (plus the daemon pid line read from the pid file).
/// Refresh until interrupted (Ctrl-C). Report clearly when no daemon is running. See
/// `specs/cli.md` and `specs/daemon.md`.
pub fn status() -> Result<()> {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    loop {
        // Clear screen and move cursor to top-left.
        write!(out, "\x1b[2J\x1b[H")?;

        render_status(&mut out)?;
        out.flush()?;

        std::thread::sleep(Duration::from_secs(1));
    }
}

fn render_status<W: Write>(out: &mut W) -> Result<()> {
    let now = SystemTime::now();

    // Read the pid file to determine whether the daemon is alive.
    match pidfile::read_live() {
        Some(pid) => {
            writeln!(out, "shpi daemon: running (pid {pid})")?;
        }
        None => {
            writeln!(out, "shpi daemon: not running")?;
            return Ok(());
        }
    }

    // Query the control socket.
    let reports = match query_control_socket() {
        Ok(r) => r,
        Err(_) => {
            // Socket unreachable — daemon may be starting up.
            return Ok(());
        }
    };

    for h in &reports {
        let addr_col = format!("  {}", h.addr);
        match &h.status {
            TunnelStatus::Connected { since } => {
                let elapsed = now.duration_since(*since).unwrap_or(Duration::ZERO);
                writeln!(
                    out,
                    "{addr_col:<24} {:<14}({})",
                    "connected",
                    fmt_duration(elapsed)
                )?;
            }
            TunnelStatus::Reconnecting {
                attempt,
                next_retry,
            } => {
                let remaining = next_retry.duration_since(now).unwrap_or(Duration::ZERO);
                writeln!(
                    out,
                    "{addr_col:<24} {:<14}(attempt {attempt}, next retry in {})",
                    "reconnecting",
                    fmt_duration(remaining)
                )?;
            }
            TunnelStatus::Failed { reason } => {
                writeln!(out, "{addr_col:<24} {:<14}({reason})", "failed")?;
            }
        }
    }

    Ok(())
}

/// Open the control socket, read the full reply, and decode it.
fn query_control_socket() -> Result<Vec<crate::state::HostReport>> {
    let sock_path = paths::control_sock()?;
    let mut stream = UnixStream::connect(&sock_path)
        .map_err(|e| Error::msg(format!("could not connect to control socket: {e}")))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(protocol::decode_report(&reply))
}

// ─── stop ────────────────────────────────────────────────────────────────────

/// `shpi stop`: read the pid file, SIGTERM the daemon, escalate to SIGKILL after a short
/// grace period if still alive, remove the pid file if it remains. Succeed quietly if no
/// daemon is running. See `specs/daemon.md`.
pub fn stop() -> Result<()> {
    // No (parseable) pid file, or a stale one — nothing to stop.
    let pid = match pidfile::read() {
        Some(p) => p,
        None => return Ok(()),
    };
    if !pidfile::is_alive(pid) {
        pidfile::remove();
        return Ok(());
    }

    // Send SIGTERM, then poll for up to ~2 s (20 × 100 ms).
    let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
    let mut alive = true;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if !pidfile::is_alive(pid) {
            alive = false;
            break;
        }
    }

    if alive {
        // Escalate to SIGKILL, then give the kernel a moment to reap the process.
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
        std::thread::sleep(Duration::from_millis(100));
    }

    pidfile::remove();
    Ok(())
}

// ─── duration formatting ─────────────────────────────────────────────────────

/// Format a [`Duration`] as a compact human string: `3h 22m`, `45s`, `2d 4h`, etc.
///
/// Rules:
/// - `>= 1 day`:   `{d}d {h}h` (hours component omitted when zero).
/// - `>= 1 hour`:  `{h}h {m}m` (minutes component omitted when zero).
/// - `>= 1 min`:   `{m}m {s}s` (seconds component omitted when zero).
/// - otherwise:    `{s}s`.
fn fmt_duration(d: Duration) -> String {
    let total = d.as_secs();
    let days = total / 86400;
    let hours = (total % 86400) / 3600;
    let mins = (total % 3600) / 60;
    let secs = total % 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if mins > 0 {
            format!("{hours}h {mins}m")
        } else {
            format!("{hours}h")
        }
    } else if mins > 0 {
        if secs > 0 {
            format!("{mins}m {secs}s")
        } else {
            format!("{mins}m")
        }
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_duration_samples() {
        assert_eq!(fmt_duration(Duration::from_secs(0)), "0s");
        assert_eq!(fmt_duration(Duration::from_secs(45)), "45s");
        assert_eq!(fmt_duration(Duration::from_secs(60)), "1m");
        assert_eq!(fmt_duration(Duration::from_secs(65)), "1m 5s");
        assert_eq!(fmt_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(fmt_duration(Duration::from_secs(3600 + 22 * 60)), "1h 22m");
        assert_eq!(
            fmt_duration(Duration::from_secs(3 * 3600 + 22 * 60)),
            "3h 22m"
        );
        assert_eq!(
            fmt_duration(Duration::from_secs(2 * 86400 + 4 * 3600)),
            "2d 4h"
        );
        assert_eq!(fmt_duration(Duration::from_secs(2 * 86400)), "2d");
    }
}
