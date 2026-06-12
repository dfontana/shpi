# Request Lifecycle Specification

A "request" is a single message traveling from a remote host's `portpipe send` to a
`watch` consumer on the Mac. A message carries a body (the text being sent) and the
sender's hostname as metadata. Messages are one-way; there is no reply.

## Frame format

Each message is one self-delimiting frame:

```
[ frame_len: u32 ][ host_len: u16 ][ host bytes ][ body bytes ]
```

- `frame_len` (big-endian) is the byte count of everything after it (`2 + host_len + body_len`).
- `host_len` (big-endian) is the byte length of the hostname.
- `host` and `body` are UTF-8. `body` is the remaining `frame_len - 2 - host_len` bytes.

The explicit `frame_len` prefix is required because the FIFO carries a continuous stream
of frames with no other boundary; a reader uses it to delimit each frame. Bodies may
contain any bytes, including newlines.

## Send side

`portpipe send <message>` on a remote host:

1. Builds a frame with `host` = the sender's hostname (the system hostname by default,
   overridable via `--name`) and `body` = the message.
2. Opens a TCP connection to `127.0.0.1:PORT` — the remote end of the SSH reverse tunnel
   (see [ssh-connection](./ssh-connection.md)).
3. Writes exactly one frame and closes the connection.

If no tunnel is present the connection fails and `send` exits non-zero (see [cli](./cli.md)).

## Transit

The frame travels over the SSH tunnel from the remote host's `localhost:PORT` to the Mac
daemon's listener at `127.0.0.1:PORT`. All transit is inside the existing SSH connection;
no traffic crosses a physical interface in cleartext.

## Receive side

The daemon (see [daemon](./daemon.md)):

1. Accepts the inbound TCP connection (one frame per connection).
2. Reads `frame_len`, then the full frame. Reads may be partial; the daemon buffers until
   the whole frame is present.
3. Forwards the entire frame, unchanged and including its length prefix, to the output
   FIFO. The daemon does not parse `host` or `body` — it is a pass-through.

## Delivery to consumer

`watch` reads framed messages from the FIFO. For each frame it splits out `host` and
`body` and renders them per its output mode (see [cli](./cli.md)): a parseable `host`/`body`
line by default, or an OSC-99 notification sequence under `--osc99`. With no consumer
attached, messages are not retained (no persistence/history — out of scope), so a message
sent while nothing is watching may be discarded.
