use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use plane_core::{messages::agent::SpawnRequest, types::BackendId};
use std::pin::Pin;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EngineBackendStatus {
    /// No information about this backend is available, it may not have started.
    Unknown,

    /// The backend is running.
    Running,

    /// The backend exited on its own without indicating failure.
    Finished,

    /// The backend exited on its own with a failure state.
    Failed,

    /// The backend was terminated by external forces.
    Terminated,
}

#[async_trait]
pub trait Engine {
    /// Returns an async stream which yields a backend ID when the status of that
    /// backend has changed. This causes the Plane-side control loop to request
    /// state information from the engine.
    fn interrupt_stream(&self) -> Pin<Box<dyn Stream<Item = BackendId> + Send>>;

    /// Load resources for a backend.
    async fn load(&self, spawn_request: &SpawnRequest) -> Result<()>;

    /// Return true if the backend is running according to the execution engine.
    /// This is considered a necessary but not sufficient condition for the
    /// backend to be considered "ready" by the agent.
    async fn backend_status(&self, container_name: &BackendId) -> Result<EngineBackendStatus>;
}
