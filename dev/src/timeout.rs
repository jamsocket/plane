use anyhow::Result;
use std::{
    future::Future,
    time::{Duration, SystemTime}, fmt::Display,
};
use tokio::task::JoinHandle;

struct HumanDuration {
    millis: u128
}

const SECOND: u128 = 1_000;
const MINUTE: u128 = SECOND * 60;

impl HumanDuration {
    fn stringify(&self) -> String {
        if self.millis > MINUTE {
            let mins = self.millis as f64 / MINUTE as f64;
            format!("{:.2}m", mins)
        } else if self.millis > SECOND {
            let secs = self.millis as f64 / SECOND as f64;
            format!("{:.2}s", secs)
        } else {
            format!("{}ms", self.millis)
        }
    }

    pub fn new(millis: u128) -> Self {
        HumanDuration { millis }
    }

    pub fn from_duration(duration: Duration) -> Self {
        HumanDuration { millis: duration.as_millis() }
    }
}

impl Display for HumanDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.stringify())
    }
}

/// If the given future resolves before the given timeout, its value
/// is returned. Otherwise, panics.
///
/// This returns a future which must be awaited. If you want to put a
/// timeout on a future that runs “in the background” without being awaited,
/// use [spawn_timeout].
pub async fn timeout<F, T>(timeout_ms: u64, message: &str, future: F) -> T
where
    F: Future<Output = T>,
{
    let start_time = SystemTime::now();
    let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), future).await {
        Ok(t) => t,
        Err(_) => panic!("{} timed out after {}ms", message, timeout_ms),
    };
    let elapsed = SystemTime::now()
        .duration_since(start_time)
        .unwrap();

    tracing::info!(
        timeout = %HumanDuration::new(timeout_ms as _),
        elapsed = %HumanDuration::from_duration(elapsed),
        message,
        "Successfully executed future before timeout."
    );

    result
}

#[must_use]
pub fn spawn_timeout<F>(
    timeout_ms: u64,
    message: &str,
    future: F,
) -> tokio::task::JoinHandle<Result<()>>
where
    F: Future<Output = Result<()>> + Send + Sync + 'static,
{
    let message = message.to_owned();

    tokio::spawn(async move {
        let result = tokio::time::timeout(Duration::from_millis(timeout_ms), future).await?;

        match result {
            Err(_) => panic!("{} timed out after {}", message, HumanDuration::new(timeout_ms as _)),
            Ok(v) => Ok(v),
        }
    })
}

pub struct LivenessGuard {
    #[allow(unused)]
    handle: JoinHandle<Result<()>>,
}

impl Drop for LivenessGuard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Takes a future that you expect to never resolve (e.g. a server that awaits
/// messages on an infinite loop). Returns a guard. If the future resolves before
/// the guard goes out of scope, panics.
///
/// The future is aborted when the guard is dropped.
pub fn expect_to_stay_alive<F>(future: F) -> LivenessGuard
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    let handle = tokio::spawn(async {
        future.await?;

        panic!("Expected future to stay alive until we killed it, but it finished on its own.");
    });

    LivenessGuard { handle }
}
