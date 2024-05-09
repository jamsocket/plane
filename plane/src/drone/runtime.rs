use super::docker::{SpawnResult, TerminateEvent};
use crate::{
    database::backend::BackendMetricsMessage, names::BackendName, protocol::AcquiredKey,
    types::BearerToken,
};
use anyhow::Error;
use futures_util::Stream;

#[allow(async_fn_in_trait)]
pub trait Runtime: Sync {
    type RuntimeConfig;
    type BackendConfig: Clone + Send + Sync;

    async fn prepare(&self, config: &Self::BackendConfig) -> Result<(), Error>;

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: Self::BackendConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult, Error>;

    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<(), Error>;

    /// Provides a callback to be called when the executor has a new metrics message for
    /// any backend.
    fn metrics_callback<F: Fn(BackendMetricsMessage) + Send + Sync + 'static>(&self, sender: F);

    fn events(&self) -> impl Stream<Item = TerminateEvent>;
}
