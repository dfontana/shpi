# CLI Specification

`portpipe` is a single binary with two roles: a receiver (`start`/`stop`/`status`/`watch`)
run on the Mac, and a sender (`send`) run on a remote Linux host.

## Commands

```
portpipe
├── start <user@host>...   [--port] [--log]   # Mac side
├── stop
├── status
├── watch                  [--osc99]
└── send <message>         [--port] [--name]   # remote Linux side
```

### `portpipe start [--port <PORT>] [--log <PATH>] <user@host>...`

Starts the receiver daemon in the background and establishes one SSH tunnel per
`user@host` argument. At least one host is required.

- `--port <PORT>` — TCP port used for both the local listener and the remote forwarded
  port. Default: `9292`.
- `--log <PATH>` — daemon log file location. Default: a fixed path under the portpipe
  data directory.

Fails with a distinct, clear error when:
- A portpipe daemon is already running.
- The port is already bound by a non-portpipe process.

On success the command returns promptly after the daemon has detached; tunnel
establishment continues asynchronously in the background.

### `portpipe stop`

Stops the running daemon. Terminates the daemon and all SSH tunnel processes it owns.
Succeeds quietly if no daemon is running.

### `portpipe status`

Prints human-readable per-host tunnel state and whether the daemon is running. Queries
the running daemon over its control socket for live state and renders it. If no daemon is
reachable, reports that it is not running. Example shape:

```
portpipe daemon: running (pid 12345)
  user@host1    connected       (3h 22m)
  user@host2    reconnecting    (attempt 3, next retry in 45s)
  user@host3    failed          (host unreachable)
```

Reports clearly when no daemon is running. This view should refresh itself until the user terminates the command (ctrl+c)

### `portpipe watch`

Streams messages received by the daemon to stdout as they arrive, flushing after each
message so it reaches a downstream pipeline immediately even when stdout is a pipe rather
than a terminal. Blocks until interrupted. Multiple concurrent `watch` invocations are
permitted (see the FIFO distribution caveat in [daemon](./daemon.md)).

Output modes:
- Default — one parseable line per message: the hostname and body separated by a tab.
- `--osc99` — render each message as an OSC-99 notification escape sequence with the
  hostname as the title and the body as the message:
  `\033]99;;<hostname>;<body>\033\\`.

### `portpipe send [--port <PORT>] [--name <NAME>] <message>`

Run on a remote Linux host. Sends a single message to the receiver through the SSH
tunnel, which is always on localhost. The message frame carries the sender's hostname as
metadata (see [requests](./requests.md)).

- `--port <PORT>` — port to connect to. Default: `9292`.
- `--name <NAME>` — hostname to attach to the message. Default: the system hostname.

Exits non-zero with a clear error if the connection cannot be established (e.g. no tunnel
present).

## Conventions

- All commands exit `0` on success, non-zero on failure, with errors written to stderr.
- The send/recv default ports must match for a message to be delivered.
