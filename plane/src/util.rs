use chrono::Duration;
use futures_util::Future;
use std::{
    net::{IpAddr, ToSocketAddrs},
};
use tokio::task::JoinHandle;

pub fn format_duration(duration: Duration) -> String {
    let mut parts = vec![];

    let days = duration.num_days();
    if days > 0 {
        parts.push(format!("{}d", days));
    }

    let hours = duration.num_hours() % 24;
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }

    let minutes = duration.num_minutes() % 60;
    if minutes > 0 {
        parts.push(format!("{}m", minutes));
    }

    let seconds = duration.num_seconds() % 60;
    if seconds > 0 {
        parts.push(format!("{}s", seconds));
    }

    if parts.is_empty() {
        "0s".to_string()
    } else {
        parts.join(" ")
    }
}

#[derive(Debug)]
pub struct GuardHandle {
    handle: JoinHandle<()>,
}

impl GuardHandle {
    pub fn new<F>(future: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        Self { handle }
    }
}

impl Drop for GuardHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Resolve a hostname to an IP address.
pub fn resolve_hostname(hostname: &str) -> Option<IpAddr> {
    // The port is arbitrary, but needs to be provided.
    let socket_addrs = format!("{}:0", hostname).to_socket_addrs().ok()?;

    for socket_addr in socket_addrs {
        if let IpAddr::V4(ip) = socket_addr.ip() {
            tracing::info!("Resolved hostname to IP: {}", ip);
            return Some(ip.into());
        }
    }

    None
}
