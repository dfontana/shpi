//! The daemon pid file: reading it and probing whether that process is alive.
//!
//! One owner for the "read the pid, is it alive, drop a stale file" sequence that
//! the daemon's start guard, `status`, and `stop` all need. The file itself is
//! written by `daemonize`; this module only reads and removes it.

use crate::paths;
use nix::unistd::Pid;

/// Probe whether `pid` is alive using `kill(pid, 0)`, which sends no signal.
pub fn is_alive(pid: Pid) -> bool {
    nix::sys::signal::kill(pid, None).is_ok()
}

/// Read the daemon pid. `None` if the file is absent or unparseable.
pub fn read() -> Option<Pid> {
    let path = paths::pid_file().ok()?;
    let contents = std::fs::read_to_string(path).ok()?;
    let raw: i32 = contents.trim().parse().ok()?;
    Some(Pid::from_raw(raw))
}

/// Read the pid only if that process is currently alive.
pub fn read_live() -> Option<Pid> {
    let pid = read()?;
    is_alive(pid).then_some(pid)
}

/// Remove the pid file (best effort; ignores a missing file).
pub fn remove() {
    if let Ok(path) = paths::pid_file() {
        let _ = std::fs::remove_file(path);
    }
}
