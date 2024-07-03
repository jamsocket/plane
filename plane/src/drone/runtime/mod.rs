use crate::{
    database::backend::BackendMetricsMessage,
    names::BackendName,
    protocol::AcquiredKey,
    types::{backend_state::BackendError, BearerToken},
};
use anyhow::Error;
use docker::{SpawnResult, TerminateEvent};
use futures_util::Stream;
use serde::{de::DeserializeOwned, Serialize};
use std::{net::SocketAddr, pin::Pin};

pub mod docker;
#[allow(unused)] // for now, to disable clippy noise
pub mod unix_socket;

#[async_trait::async_trait]
pub trait Runtime: Send + Sync + 'static {
    type RuntimeConfig;
    type BackendConfig: Clone + Send + Sync + 'static + DeserializeOwned + Serialize;

    async fn prepare(&self, config: &Self::BackendConfig) -> Result<(), Error>;

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: Self::BackendConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult, Error>;

    /// Attempts to terminate the backend. If `hard` is true, the runtime should make every effort
    /// to forcibly terminate the backend immediately; otherwise it should invoke graceful shutdown.
    /// If the backend is already terminated or does not exist, this should return `Ok(false)`.
    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<bool, Error>;

    /// Provides a callback to be called when the executor has a new metrics message for
    /// any backend.
    fn metrics_callback(&self, sender: Box<dyn Fn(BackendMetricsMessage) + Send + Sync + 'static>);

    fn events(&self) -> Pin<Box<dyn Stream<Item = TerminateEvent> + Send>>;

    async fn wait_for_backend(
        &self,
        backend: &BackendName,
        address: SocketAddr,
    ) -> Result<(), BackendError>;
}
