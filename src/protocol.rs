//! Text encoding for the control-socket status reply.
//!
//! One host per line, tab-separated fields. Absolute instants are encoded as unix
//! seconds so the `status` client renders durations at read time.
//!
//! - `addr \t connected \t <since_unix_secs>`
//! - `addr \t reconnecting \t <attempt> \t <next_retry_unix_secs>`
//! - `addr \t failed \t <reason>`

use crate::state::{HostReport, TunnelStatus};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn to_unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn from_unix(s: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(s)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\t' || c == '\n' { ' ' } else { c })
        .collect()
}

pub fn encode_report(hosts: &[HostReport]) -> String {
    let mut out = String::new();
    for h in hosts {
        let line = match &h.status {
            TunnelStatus::Connected { since } => {
                format!("{}\tconnected\t{}", sanitize(&h.addr), to_unix(*since))
            }
            TunnelStatus::Reconnecting {
                attempt,
                next_retry,
            } => format!(
                "{}\treconnecting\t{}\t{}",
                sanitize(&h.addr),
                attempt,
                to_unix(*next_retry)
            ),
            TunnelStatus::Failed { reason } => {
                format!("{}\tfailed\t{}", sanitize(&h.addr), sanitize(reason))
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn decode_report(s: &str) -> Vec<HostReport> {
    let mut hosts = Vec::new();
    for line in s.lines() {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 2 {
            continue;
        }
        let addr = f[0].to_string();
        let status = match f[1] {
            "connected" => TunnelStatus::Connected {
                since: from_unix(f.get(2).and_then(|x| x.parse().ok()).unwrap_or(0)),
            },
            "reconnecting" => TunnelStatus::Reconnecting {
                attempt: f.get(2).and_then(|x| x.parse().ok()).unwrap_or(0),
                next_retry: from_unix(f.get(3).and_then(|x| x.parse().ok()).unwrap_or(0)),
            },
            _ => TunnelStatus::Failed {
                reason: f.get(2).copied().unwrap_or("").to_string(),
            },
        };
        hosts.push(HostReport { addr, status });
    }
    hosts
}
