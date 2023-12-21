use chrono::Duration;
use futures_util::Future;
use rand::{
    distributions::{Distribution, Uniform},
    Rng,
};
use std::{
    net::{IpAddr, ToSocketAddrs},
    time::SystemTime,
};
use tokio::task::JoinHandle;

const ALLOWED_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

pub fn random_string() -> String {
    let range = Uniform::new(0, ALLOWED_CHARS.len());

    let mut rng = rand::thread_rng();

    range
        .sample_iter(&mut rng)
        .take(14)
        .map(|i| ALLOWED_CHARS.chars().nth(i).unwrap())
        .collect()
}

pub fn random_token() -> String {
    let mut token = [0u8; 32];
    rand::thread_rng().fill(&mut token);
    data_encoding::BASE64URL_NOPAD.encode(&token)
}

pub fn random_prefixed_string(prefix: &str) -> String {
    format!("{}-{}", prefix, random_string())
}

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

pub struct ExponentialBackoff {
    initial_duration_millis: i64,
    max_duration: Duration,
    defer_duration: Duration,
    multiplier: f64,
    step: i32,
    deferred_reset: Option<SystemTime>,
}

impl ExponentialBackoff {
    fn new(
        initial_duration: Duration,
        max_duration: Duration,
        multiplier: f64,
        defer_duration: Duration,
    ) -> Self {
        let initial_duration_millis = initial_duration.num_milliseconds();

        Self {
            initial_duration_millis,
            max_duration,
            multiplier,
            step: 0,
            defer_duration,
            deferred_reset: None,
        }
    }

    /// Reset the backoff, but only if `wait` is not called again for at least `defer_duration`.
    pub fn defer_reset(&mut self) {
        self.deferred_reset = Some(
            SystemTime::now()
                + self
                    .defer_duration
                    .to_std()
                    .expect("defer_duration is always valid"),
        );
    }

    pub async fn wait(&mut self) {
        if let Some(deferred_reset) = self.deferred_reset {
            self.deferred_reset = None;
            if SystemTime::now() > deferred_reset {
                self.reset();
                return;
            }
        }

        let duration = self.initial_duration_millis as f64 * self.multiplier.powi(self.step);
        let duration = Duration::milliseconds(duration as i64);
        let duration = duration.min(self.max_duration);

        tokio::time::sleep(duration.to_std().expect("duration is always valid")).await;

        self.step += 1;
    }

    pub fn reset(&mut self) {
        self.deferred_reset = None;
        self.step = 0;
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new(
            Duration::seconds(1),
            Duration::minutes(1),
            1.1,
            Duration::minutes(1),
        )
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

pub fn get_internal_host_ip() -> Option<IpAddr> {
    // The port is arbitrary, but needs to be provided.
    let socket_addrs = "host.docker.internal:0".to_socket_addrs().ok()?;

    for socket_addr in socket_addrs {
        if let IpAddr::V4(ip) = socket_addr.ip() {
            return Some(ip.into());
        }
    }

    None
}

pub struct Callback<T: Send + Sync, E: Send + Sync> {
    func: Box<dyn Fn(T) -> Result<(), E> + Send + Sync + 'static>,
    description: &'static str,
}

impl<T: Send + Sync, E: Send + Sync> std::fmt::Debug for Callback<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description)
    }
}

impl<T: Send + Sync, E: Send + Sync> Callback<T, E> {
    pub fn new<C: Into<&'static str>>(
        value: impl Fn(T) -> Result<(), E> + Send + Sync + 'static,
        description: C,
    ) -> Self {
        Self {
            func: Box::new(value),
            description: description.into(),
        }
    }

    pub fn call(&self, inp: T) -> Result<(), E> {
        (self.func)(inp)
    }
}
