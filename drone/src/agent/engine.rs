use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use plane_core::{
    messages::agent::{BackendStatsMessage, DroneLogMessage, SpawnRequest},
    types::{BackendId, ClusterName, DroneId},
};
use std::{net::SocketAddr, pin::Pin};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EngineBackendStatus {
    /// No information about this backend is available, it may not have started.
    Unknown,

    /// The backend is running.
    Running { addr: SocketAddr },

    /// The backend exited on its own without indicating failure.
    Exited,

    /// The backend exited on its own with a failure state.
    Failed { code: u16 },

    /// The backend was terminated by external forces.
    Terminated,
}

#[async_trait]
pub trait Engine: Send + Sync + 'static {
    /// Returns an async stream which yields a backend ID when the status of that
    /// backend has changed. This causes the Plane-side control loop to request
    /// state information from the engine.
    fn interrupt_stream(&self) -> Pin<Box<dyn Stream<Item = BackendId> + Send>>;

    /// Load resources for a backend.
    async fn load(&self, spawn_request: &SpawnRequest) -> Result<()>;

    /// Return true if the backend is running according to the execution engine.
    /// This is considered a necessary but not sufficient condition for the
    /// backend to be considered "ready" by the agent.
    async fn backend_status(&self, spawn_request: &SpawnRequest) -> Result<EngineBackendStatus>;

    /// Terminate a backend.
    async fn stop(&self, backend: &BackendId) -> Result<()>;

    fn log_stream(
        &self,
        backend: &BackendId,
    ) -> Pin<Box<dyn Stream<Item = DroneLogMessage> + Send>>;

    fn stats_stream(
        &self,
        backend: &BackendId,
        drone: &DroneId,
        cluster: &ClusterName,
    ) -> Pin<Box<dyn Stream<Item = BackendStatsMessage> + Send>>;
}
