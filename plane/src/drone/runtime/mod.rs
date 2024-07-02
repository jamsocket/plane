use crate::{
    database::backend::BackendMetricsMessage,
    names::BackendName,
    protocol::AcquiredKey,
    types::{backend_state::BackendError, BearerToken},
};
use anyhow::Error;
use docker::{SpawnResult, TerminateEvent};
use futures_util::{Future, Stream};
use serde::{de::DeserializeOwned, Serialize};
use std::net::SocketAddr;

pub mod docker;
#[allow(unused)] // for now, to disable clippy noise
pub mod unix_socket;

pub trait Runtime: Send + Sync + 'static {
    type RuntimeConfig;
    type BackendConfig: Clone + Send + Sync + 'static + DeserializeOwned + Serialize;

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

    /// Attempts to terminate the backend. If `hard` is true, the runtime should make every effort
    /// to forcibly terminate the backend immediately; otherwise it should invoke graceful shutdown.
    /// If the backend is already terminated or does not exist, this should return `Ok(false)`.
    fn terminate(
        &self,
        backend_id: &BackendName,
        hard: bool,
    ) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Provides a callback to be called when the executor has a new metrics message for
    /// any backend.
    fn metrics_callback<F: Fn(BackendMetricsMessage) + Send + Sync + 'static>(&self, sender: F);

    fn events(&self) -> impl Stream<Item = TerminateEvent> + Send;

    fn wait_for_backend(
        &self,
        backend: &BackendName,
        address: SocketAddr,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;
}
