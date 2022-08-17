use std::{
    future::Future,
    time::{Duration, SystemTime},
};

use anyhow::Result;
use tokio::time::error::Elapsed;

pub async fn timeout<F, T>(timeout_ms: u64, message: &str, future: F) -> T
where
    F: Future<Output = T>,
{
    let start_time = SystemTime::now();
    let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), future).await {
        Ok(t) => t,
        Err(_) => panic!("{} timed out after {}ms", message, timeout_ms),
    };
    let elapsed_ms = SystemTime::now()
        .duration_since(start_time)
        .unwrap()
        .as_millis();

    tracing::info!(
        ?timeout_ms,
        elapsed_ms,
        message,
        "Successfully executed future before timeout."
    );

    result
}

#[must_use]
pub fn spawn_timeout<F>(
    timeout_ms: u64,
    future: F,
) -> tokio::task::JoinHandle<std::result::Result<Result<()>, Elapsed>>
where
    F: Future<Output = Result<()>> + Send + Sync + 'static,
{
    tokio::spawn(tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        future,
    ))
}
