# Daemon Specification

The receiver daemon runs on the Mac. It is started by `start`, detaches from the
controlling terminal, and runs until stopped. It owns the TCP listener that receives
messages, the SSH tunnels (see [ssh-connection](./ssh-connection.md)), the control socket
that answers `status` queries, and the FIFO through which `watch` consumes output.

## Data directory

The daemon owns a single data directory (under the user's $HOME/.cache/ directory). It contains:

| File | Purpose |
|---|---|
| `portpipe.pid` | PID of the running daemon. Written at start, removed at stop. |
| `output.pipe` | FIFO carrying received messages to `watch` consumers. |
| `control.sock` | Unix-domain socket the daemon serves to answer `status` queries. |
| `portpipe.log` | Daemon log (also the daemon's redirected stdout/stderr). |

The directory, FIFO, and control socket are created if absent during startup. A stale
control socket left by a prior unclean exit must be removed and re-bound on startup.

## Startup

`start` performs, in order:

1. Refuse to start if a live daemon already exists (a PID file whose process is alive).
   A stale PID file (no live process) must not block startup.
2. Ensure the data directory and the FIFO exist.
3. Detach into the background and redirect stdout/stderr to the log file.
4. Write the PID file.
5. Bind the TCP listener to `127.0.0.1:PORT`. Binding to loopback only is required — no
   message traffic touches a physical interface.
6. Bind the control socket.
7. Begin accepting connections, serve the control socket, start one SSH tunnel monitor per
   host, and install the signal handler.

If the port is already in use, startup fails with an error distinguishing "portpipe
already running" from "another process holds the port" (see [cli](./cli.md)).

## Receiving connections

The daemon accepts many inbound TCP connections concurrently and continuously; one
slow or long-lived connection must not block others. Each connection's bytes are decoded
into messages per the framing in [requests](./requests.md) and written to the FIFO.

## Output FIFO

Received messages are written, line by line, to `output.pipe`. `watch` reads from it.

Caveat: a FIFO open blocks until both a reader and a writer are present, and writes to a
FIFO with no reader fail. The daemon must keep the FIFO usable regardless of whether any
`watch` is currently running — it must neither block startup waiting for a reader nor
crash when no reader is attached. (Holding the FIFO open for both reading and writing on
the daemon side satisfies this.)

When no consumer is reading, messages may be dropped rather than buffered indefinitely;
there is no persistence or history (out of scope).

## Status queries

The daemon holds live per-host tunnel state in memory and answers queries on the control
socket. A `status` client connects, the daemon replies with a snapshot of current state,
and the connection closes. Each reply describes, per host: its address, connection status
(connected / reconnecting / failed), the time the current connection was established (while
connected), and the retry attempt and next-retry time (while reconnecting). Absolute
instants are reported so the client renders exact elapsed/remaining durations at read time.

Serving the control socket must not block the daemon's other work, and a slow or stalled
client must not stall message receiving or tunnel monitors.

## Shutdown

`stop` signals the daemon to terminate (SIGTERM), then escalates to SIGKILL if the
process does not exit within a short grace period, and removes the PID file if it remains.

On receiving SIGTERM the daemon must kill every SSH tunnel child it spawned before
exiting. Orphaned SSH processes can hold the remote forwarded port and cause the next
daemon start to fail its forward binding — graceful child cleanup is required for restarts
to succeed. The daemon also removes the PID file and control socket on clean exit; a stale
socket from an unclean exit is handled at the next startup.

## Concurrency caveat

The daemon performs concurrent I/O-bound work: accepting connections, reading sockets,
and waiting on SSH child processes. Waiting on a child process must not block the progress
of unrelated tasks (e.g. accepting new connections or other tunnels' monitors).
