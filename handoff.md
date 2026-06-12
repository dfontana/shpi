# portpipe — Claude Code Handoff Document

## What This Is

A small Rust CLI tool for sending UTF-8 string data from one or more remote Linux hosts to a Mac listener over SSH reverse tunnels. The Mac side daemonizes and manages its own SSH tunnel persistence. The Linux side just sends strings.

No HTTP framework. No broker. Raw TCP sockets + SSH tunnels.

---

## Problem Being Solved

The user SSHes into multiple Linux hosts and wants a low-friction way to pipe string data (log lines, notifications, status messages, etc.) back to their Mac. The Mac is the stable endpoint; Linux hosts are ephemeral. The tunnel is established and kept alive by the Mac daemon, not by autossh or systemd on the Linux side.

---

## Architecture Overview

### One binary, two roles

```
portpipe recv start [user@host1] [user@host2] ...
portpipe recv stop
portpipe recv status
portpipe recv watch              # streams received messages to stdout

portpipe send "some message"     # run on Linux side
```

### Transport

- **TCP sockets** over **SSH remote port forwarding**
- On Mac: `TcpListener` bound to `127.0.0.1:PORT`
- The Mac daemon shells out to `ssh -NR PORT:localhost:PORT user@host` for each configured remote host
- Linux hosts send to `localhost:PORT` — they never need to know the Mac's IP
- All traffic stays inside the existing SSH connection

### Why loopback-only

The Mac listener binds to `127.0.0.1`, not `0.0.0.0`. Tunneled traffic arrives from loopback. No firewall rules needed on either side. No port exposure on any physical interface.

---

## Daemon Architecture

### Stdout / pipeline problem

Daemons have no terminal. But `recv watch` needs to be pipeable (`portpipe recv watch | grep foo`).

**Solution: named pipe (FIFO)**
- Daemon writes received messages to `~/.portpipe/output.pipe`
- `recv watch` opens that FIFO for reading and copies to its own stdout
- Clean unix composition; daemon and watcher are decoupled

### Files the daemon manages

```
~/.portpipe/
  portpipe.pid          # written at start, deleted at stop
  output.pipe           # FIFO for received messages
  status.json           # per-host tunnel state, written periodically
  portpipe.log          # daemon log
```

### Concurrency model

Use **smol** (not Tokio) — async/await ergonomics without the heavy machinery. This is I/O-bound work (socket reads, process waits) so async is a natural fit.

Three categories of concurrent tasks inside the daemon:

1. **TCP accept loop** — `smol::net::TcpListener`, spawns a task per inbound connection to read bytes and write lines to the FIFO
2. **One SSH monitor task per host** — wait/respawn loop (see below)
3. **Signal handler** — SIGTERM: kill child SSH processes, remove pid file, clean up FIFO

### SSH monitor task (per host)

```
spawn ssh child
track connection_start = Instant::now()

loop:
    async wait for child to exit   // smol::process — do NOT use blocking wait()
    log exit code
    if connection lasted > HEALTHY_THRESHOLD:
        reset backoff
    sleep(backoff)
    backoff = min(backoff * 2, MAX_BACKOFF)   // cap at ~60s
    respawn ssh child
    connection_start = Instant::now()
```

**Key detail:** use `ServerAliveInterval` and `ServerAliveCountMax` as `-o` flags on the SSH command so SSH itself detects dead connections and exits cleanly — the monitor loop doesn't poll, it just waits for exit.

```
ssh -NR {port}:localhost:{port} \
    -o ServerAliveInterval=30 \
    -o ServerAliveCountMax=3 \
    -o ExitOnForwardFailure=yes \
    {user@host}
```

`ExitOnForwardFailure=yes` makes SSH exit immediately if it can't bind the remote port, rather than connecting but silently not forwarding — important for the monitor loop to work correctly.

### State the daemon holds

```rust
struct HostState {
    addr: String,               // "user@host"
    child: Option<Child>,       // current ssh process handle
    status: TunnelStatus,       // Connected | Reconnecting | Failed
    backoff: Duration,
    connected_since: Option<Instant>,
}

enum TunnelStatus {
    Connected,
    Reconnecting { attempt: u32, next_retry: Instant },
    Failed(String),
}
```

---

## Wire Protocol

TCP is a byte stream with no message boundaries. Framing strategy: **newline-terminated UTF-8 strings**.

- Sender writes `"message\n"` 
- Receiver reads until `\n`, treats each line as one message
- This makes `portpipe send` trivially composable: `echo "hello" | portpipe send -` could also be a nice addition

The daemon should handle partial reads correctly — buffer until newline is seen.

---

## Crate Dependencies

| Crate | Purpose |
|---|---|
| `clap` | CLI argument parsing (derive API) |
| `daemonize` | Fork to background, write pid file |
| `smol` | Async executor |
| `smol-process` or `async-process` | Async child process management (critical — don't block the executor on `Child::wait()`) |
| `serde` + `serde_json` | status.json serialization |
| `nix` | POSIX signals, mkfifo |

For `async-process`: this is the smol-ecosystem crate for async child process management. Verify it's compatible with the smol version chosen.

---

## Suggested Module Structure

```
src/
  main.rs           # clap definitions, subcommand dispatch
  daemon.rs         # daemonize, pid file, signal handling, task orchestration
  tunnel.rs         # SSH child management, monitor loop, backoff logic
  listener.rs       # TcpListener accept loop, per-connection read task, FIFO writer
  fifo.rs           # named pipe creation, watch reader (for `recv watch`)
  status.rs         # status.json read/write, `recv status` command logic
  send.rs           # sender-side TCP connect + write (Linux side logic)
  error.rs          # unified error type
```

---

## CLI Specification (clap)

```
portpipe recv start [--port <PORT>] [--log <PATH>] <user@host>...
portpipe recv stop
portpipe recv status
portpipe recv watch

portpipe send [--port <PORT>] [--host <HOST>] <message>
```

Defaults:
- `--port`: something reasonable like `9292` (avoid well-known ports)
- `--host` (send side): `127.0.0.1`

---

## `recv start` Startup Sequence

1. Validate no existing daemon (check pid file, verify process is alive)
2. Create `~/.portpipe/` directory if absent
3. Create FIFO at `~/.portpipe/output.pipe` if absent (`nix::unistd::mkfifo`)
4. Call `daemonize` crate to fork, detach, redirect stdio to log file
5. Write pid file
6. Bind `TcpListener` to `127.0.0.1:PORT`
7. Spawn TCP accept loop task
8. Spawn one SSH monitor task per host provided
9. Spawn signal handler task
10. Block on smol executor

---

## `recv stop` Logic

1. Read pid file
2. Send SIGTERM to that pid (`nix::sys::signal::kill`)
3. Wait briefly, check if gone, send SIGKILL if not
4. Remove pid file if still present

---

## `recv status` Output

Read `~/.portpipe/status.json`. Print human-readable per-host status:

```
portpipe daemon: running (pid 12345)
  user@host1    connected       (3h 22m)
  user@host2    reconnecting    (attempt 3, next retry in 45s)
  user@host3    failed          (host unreachable)
```

---

## `send` Logic (Linux side)

```rust
let mut stream = TcpStream::connect("127.0.0.1:PORT")?;
stream.write_all(format!("{}\n", message).as_bytes())?;
```

That's essentially it. The message is one write, connection is closed immediately after. The daemon handles the read side.

---

## Important Implementation Notes

1. **smol::process not std::process for wait()** — `std::process::Child::wait()` blocks the thread. Inside smol, use `async-process::Child` which integrates with the async executor properly.

2. **FIFO open blocks** — Opening a FIFO blocks until both ends are open. The daemon should open the FIFO in non-blocking mode or handle the case where no `recv watch` is running. One approach: open with `O_RDWR` on the daemon side so it never blocks waiting for a reader.

3. **ExitOnForwardFailure=yes** — Without this, SSH may connect successfully but silently fail to bind the remote port (e.g. if another process holds it). The monitor loop needs SSH to exit on failure to detect and retry.

4. **Backoff reset heuristic** — Only reset backoff if the connection stayed alive for more than ~10 seconds. Immediate exits (SSH couldn't connect at all) should keep accumulating backoff.

5. **Graceful shutdown** — On SIGTERM, the daemon should kill all SSH children before exiting. Otherwise orphaned SSH processes may hold the remote port, causing `ExitOnForwardFailure` errors on the next daemon start.

6. **Port already in use** — The `recv start` command should give a clear error if the port is already bound, distinguishing between "portpipe is already running" and "something else is on that port."

---

## What's Explicitly Out of Scope

- Encryption beyond what SSH provides (the tunnel is already encrypted)
- Authentication (loopback-only binding means only local processes can connect on the Linux side)
- Message persistence / history
- Binary payloads (UTF-8 strings only)
- Windows support
- Tokio (use smol)
- HTTP or any application-layer framing beyond newlines
