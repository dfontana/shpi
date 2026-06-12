# SSH Connection Specification

For each `user@host` configured at `start`, the daemon maintains one persistent SSH
reverse tunnel. The Mac side establishes and keeps the tunnel alive; the remote host runs
nothing persistent.

## The tunnel

Each tunnel is an SSH process configured for remote port forwarding: the remote host's
`localhost:PORT` forwards to the Mac's `127.0.0.1:PORT`. A sender on the remote host
connects to `localhost:PORT` and never needs the Mac's address. `PORT` is the same value
on both ends.

The SSH process runs no remote command (forwarding only). It must be configured so that:

- SSH itself detects a dead connection and exits, rather than hanging. This requires
  server-alive keepalive probes with a bounded count, after which SSH gives up and exits
  (`ServerAliveInterval=30`, `ServerAliveCountMax=3`).
- SSH exits immediately if it cannot bind the remote forwarded port, instead of connecting
  with a silently non-functional forward (`ExitOnForwardFailure=yes`). Without this, a held
  remote port would look like a healthy connection.

The resulting invocation has the shape:

```
ssh -NR {port}:localhost:{port} \
    -o ServerAliveInterval=30 \
    -o ServerAliveCountMax=3 \
    -o ExitOnForwardFailure=yes \
    {user@host}
```

These behaviors are required for the monitor (below) to observe failures by watching for
process exit.

## Monitoring and re-establishment

The daemon supervises each tunnel with an independent monitor that owns one SSH child at
a time:

1. Spawn the SSH child and record the time it started.
2. Wait for the child to exit. The monitor does not poll the connection; it relies on SSH's
   own keepalive to exit on failure, and waits for that exit. (Waiting on the child must not
   block other tunnels or the connection-accept path — see [daemon](./daemon.md).)
3. On exit, log the exit status and decide on backoff (below), then re-spawn.

A tunnel is considered established/connected once its SSH child is running with the forward
bound; it is reconnecting while the monitor is backing off before a respawn; it is failed
when establishment repeatedly fails.

## Backoff

Re-spawns are delayed by an exponential backoff to avoid hammering an unreachable host:

- Backoff starts small and doubles on each consecutive failure, capped at roughly 60s.
- Backoff resets to its initial value only if the previous connection stayed alive longer
  than a short health threshold (roughly 10s). A connection that exits almost immediately
  (e.g. SSH could not connect at all) must keep accumulating backoff rather than resetting.

While waiting to respawn, the monitor updates its reconnecting state — including the
attempt count and the time of the next retry — in the daemon's in-memory state, which
`status` queries over the control socket (see [daemon](./daemon.md)).

## Shutdown

When the daemon stops, every SSH child is killed (see [daemon](./daemon.md)). A monitor
must not respawn its child during shutdown.
