use anyhow::Result;
use async_trait::async_trait;
use plane_core::types::BackendId;
use serde::{de::DeserializeOwned, Serialize};
use std::net::SocketAddr;
use tokio_stream::Stream;

// pub enum BackendState {
//     Loading,
//     Starting,
//     Ready {
//         address: SocketAddr,
//     },
//     SystemError {
//         message: String,
//         code: String,
//     },
//     Finished {
//         message: String,
//         success: bool, // False if exited with error.
//     },
// }

pub enum BackendStatus {
    Running,
    Finished { message: String, success: bool },
}

/// Represents a way of executing backends.
#[async_trait]
pub trait BackendEngine: Send + Sync {
    type Spec: DeserializeOwned + Send + Sync;

    /// Load resources needed to spawn an instance of the given spec.
    async fn load(&mut self, spec: &Self::Spec) -> Result<()> {
        Ok(())
    }

    /// Spawn an instance with a given spec. On success, returns a SocketAddr on which the
    /// backend can be accessed.
    async fn start(&mut self, spec: &Self::Spec) -> Result<SocketAddr>;

    /// Terminate the instance with the given spec.
    async fn terminate(&mut self, spec: &Self::Spec) -> Result<()>;

    /// Check on the status of the instance with the given spec.
    async fn check_status(&mut self, spec: &Self::Spec) -> Result<BackendStatus>;

    /// Returns a stream which returns an interrupt based on external events. When a backend
    /// is returned in this list, check_status is called on it to determined if it needs to
    /// be terminated if it is in a running state.
    fn interrupts(&mut self) -> Box<dyn Stream<Item = BackendId>>;
}
