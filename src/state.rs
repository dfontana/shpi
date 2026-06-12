//! Shared in-memory per-host tunnel state.
//!
//! Tunnel monitors write their current status here; the control server reads a
//! snapshot to answer `status` queries. Wall-clock instants are reported so the
//! client can render exact elapsed/remaining durations at read time.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum TunnelStatus {
    Connected {
        since: SystemTime,
    },
    Reconnecting {
        attempt: u32,
        next_retry: SystemTime,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct HostReport {
    pub addr: String,
    pub status: TunnelStatus,
}

/// Cloneable shared handle to every host's current status.
#[derive(Clone)]
pub struct SharedState {
    inner: Arc<Mutex<Vec<HostReport>>>,
}

impl SharedState {
    /// Initialise one entry per host, ordered to match `addrs`.
    pub fn new(addrs: &[String]) -> Self {
        let hosts = addrs
            .iter()
            .map(|a| HostReport {
                addr: a.clone(),
                status: TunnelStatus::Reconnecting {
                    attempt: 0,
                    next_retry: SystemTime::now(),
                },
            })
            .collect();
        SharedState {
            inner: Arc::new(Mutex::new(hosts)),
        }
    }

    /// Replace the status of the host at `idx` (its position in the `addrs` slice).
    pub fn set(&self, idx: usize, status: TunnelStatus) {
        if let Ok(mut g) = self.inner.lock()
            && let Some(h) = g.get_mut(idx)
        {
            h.status = status;
        }
    }

    pub fn snapshot(&self) -> Vec<HostReport> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }
}
