use crate::NeverResult;
use std::{future::Future, time::Duration};
use tokio::task::JoinHandle;

/// A supervisor takes a factory function that returns a future, and
/// runs that future in a loop. If the future ever returns, the supervisor
/// will log an error and restart the future after a short delay.
pub struct Supervisor {
    name: &'static str,
    handle: JoinHandle<()>,
}

impl Supervisor {
    pub fn new<F, Fut>(name: &'static str, factory: F) -> Self
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = NeverResult> + Send + 'static,
    {
        let handle = tokio::spawn(async move {
            loop {
                tracing::info!(name = name, "Starting event loop");
                let fut = factory();
                let result = fut.await;
                tracing::error!(error=%result.unwrap_err(), name=name, "Event loop terminated! Restarting...");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        Self { name, handle }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        tracing::info!(name = self.name, "Shutting down supervisor");
        self.handle.abort();
    }
}
