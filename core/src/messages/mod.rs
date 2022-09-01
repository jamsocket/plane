use crate::nats::TypedNats;
use agent::{BackendStateMessage, DroneStatusMessage};
use anyhow::Result;

pub mod agent;
pub mod cert;
pub mod logging;
pub mod scheduler;

pub async fn create_streams(nats: &TypedNats) -> Result<()> {
    // Ensure that backend status stream exists.
    nats.add_jetstream_stream::<BackendStateMessage>().await?;

    // Ensure that drone status stream exists.
    nats.add_jetstream_stream::<DroneStatusMessage>().await?;

    Ok(())
}
