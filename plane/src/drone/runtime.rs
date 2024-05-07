use super::docker::{SpawnResult, TerminateEvent};
use crate::{names::BackendName, protocol::AcquiredKey, types::BearerToken};
use anyhow::Error;
use futures_util::Stream;
use std::fmt::Debug;

#[allow(async_fn_in_trait)]
pub trait Runtime: Clone + Send + Sync + Debug {
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

    async fn metrics(&self, backend_id: &BackendName) -> Result<bollard::container::Stats, Error>;

    async fn events(&self) -> impl Stream<Item = TerminateEvent>;
}
