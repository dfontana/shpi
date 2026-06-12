# Request Lifecycle Specification

A "request" is a single UTF-8 message traveling from a remote host's `portpipe send` to a
`watch` consumer on the Mac. Messages are one-way; there is no reply.

## Framing

Messages are newline-terminated UTF-8 strings. A message is the text up to (not including)
the next `\n`. This is the only message boundary; there is no other application-layer
framing. Payloads are UTF-8 text only — binary is out of scope.

## Send side

`portpipe send <message>` on a remote host:

1. Opens a TCP connection to `--host`/`--port` (default `127.0.0.1:PORT`), which is the
   remote end of the SSH reverse tunnel (see [ssh-connection](./ssh-connection.md)).
2. Writes the message followed by a single `\n`.
3. Closes the connection.

Each send is an independent, short-lived connection carrying one message. If no tunnel is
present the connection fails and `send` exits non-zero (see [cli](./cli.md)).

## Transit

The message travels over the SSH tunnel from the remote host's `localhost:PORT` to the
Mac daemon's listener at `127.0.0.1:PORT`. All transit is inside the existing SSH
connection; no traffic crosses a physical interface in cleartext.

## Receive side

The daemon (see [daemon](./daemon.md)):

1. Accepts the inbound TCP connection.
2. Reads bytes and splits them on `\n` into messages. Reads may be partial and a single
   connection may carry zero, one, or many newline-delimited messages; the daemon buffers
   incomplete data until a `\n` arrives.
3. Writes each complete message, with a trailing newline, to the output FIFO.

## Delivery to consumer

`watch` reads the FIFO and copies each message line to its own stdout. With no
consumer attached, messages are not retained (no persistence/history — out of scope), so a
message sent while nothing is watching may be discarded.
