use super::docker::{SpawnResult, TerminateEvent};
use crate::{
    database::backend::BackendMetricsMessage, names::BackendName, protocol::AcquiredKey,
    types::BearerToken,
};
use anyhow::Error;
use futures_util::{Future, Stream};
use serde::de::DeserializeOwned;

pub trait Runtime: Send + Sync + 'static {
    type RuntimeConfig;
    type BackendConfig: Clone + Send + Sync + 'static + DeserializeOwned;

    fn prepare(
        &self,
        config: &Self::BackendConfig,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    fn spawn(
        &self,
        backend_id: &BackendName,
        executable: Self::BackendConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> impl Future<Output = Result<SpawnResult, Error>> + Send;

    fn terminate(
        &self,
        backend_id: &BackendName,
        hard: bool,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Provides a callback to be called when the executor has a new metrics message for
    /// any backend.
    fn metrics_callback<F: Fn(BackendMetricsMessage) + Send + Sync + 'static>(&self, sender: F);

    fn events(&self) -> impl Stream<Item = TerminateEvent> + Send;
}
