use async_trait::async_trait;
use futures_util::Future;

#[derive(thiserror::Error, Debug)]
pub enum TimeoutError {
    #[error("Timed out after {0} seconds.")]
    TimedOut(u32),
}

#[async_trait]
pub trait WithTimeout: Sized {
    type Output;

    async fn with_timeout(self, timeout_seconds: u32) -> Result<Self::Output, TimeoutError>;
}

#[async_trait]
impl<T> WithTimeout for T
where
    T: Future + Send,
{
    type Output = T::Output;

    async fn with_timeout(self, timeout_seconds: u32) -> Result<Self::Output, TimeoutError> {
        tracing::trace!(
            "Waiting for future with timeout of {} seconds.",
            timeout_seconds
        );
        tokio::time::timeout(std::time::Duration::from_secs(timeout_seconds as u64), self)
            .await
            .map_err(|_| TimeoutError::TimedOut(timeout_seconds))
    }
}
