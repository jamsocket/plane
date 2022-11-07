use anyhow::{anyhow, Result};
use futures::FutureExt;
use std::{
    fmt::{Debug, Display},
    future::Future,
    pin::Pin,
    task::Poll,
    time::{Duration, SystemTime},
};
use tokio::task::JoinHandle;

struct HumanDuration {
    millis: u128,
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
        HumanDuration {
            millis: duration.as_millis(),
        }
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
pub async fn timeout<F, T>(timeout_ms: u64, message: &str, future: F) -> Result<T>
where
    F: Future<Output = T>,
{
    let start_time = SystemTime::now();
    let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), future).await {
        Ok(t) => t,
        Err(_) => return Err(anyhow!("{} timed out after {}ms", message, timeout_ms)),
    };
    let elapsed = SystemTime::now().duration_since(start_time).unwrap();

    tracing::info!(
        timeout = %HumanDuration::new(timeout_ms as _),
        elapsed = %HumanDuration::from_duration(elapsed),
        message,
        "Successfully executed future before timeout."
    );

    Ok(result)
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
            Err(_) => panic!(
                "{} timed out after {}",
                message,
                HumanDuration::new(timeout_ms as _)
            ),
            Ok(v) => Ok(v),
        }
    })
}

struct LivenessGuardFuture<D: Debug> {
    inner: Pin<Box<dyn Future<Output = D> + Send>>,
}

impl<D> Future for LivenessGuardFuture<D>
where
    D: Debug,
{
    type Output = D;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(val) => {
                panic!(
                    "A future wrapped in a LivenessGuard should not return a value, but did: {:?}",
                    val
                );
            }
        }
    }
}

pub struct LivenessGuard<T>
where
    T: Debug + 'static,
{
    handle: JoinHandle<T>,
}

impl<T> Drop for LivenessGuard<T>
where
    T: Debug + 'static,
{
    fn drop(&mut self) {
        self.handle.abort()
    }
}

impl<D> LivenessGuard<D>
where
    D: Debug + 'static + Send,
{
    pub fn new<F>(future: F) -> Self
    where
        F: Future<Output = D> + Send + 'static,
    {
        let handle = tokio::spawn(LivenessGuardFuture {
            inner: Box::pin(future),
        });

        Self { handle }
    }
}

pub fn expect_to_stay_alive<D, F>(future: F) -> LivenessGuard<D>
where
    F: Future<Output = D> + Send + 'static,
    D: Debug + Send,
{
    LivenessGuard::new(future)
}
