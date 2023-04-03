pub use self::state::{StateHandle, WorldState};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use plane_core::{
    messages::{
        drone_state::DroneStatusMessage,
        state::{ClusterStateMessage, DroneMessage, DroneMessageType, WorldStateMessage},
    },
    nats::{JetstreamSubscription, TypedNats},
};

mod state;

/// Start a loop which reads the state stream and applies the messages to the world state.
/// Returns a handle to the world state.
pub async fn start_state_loop(nc: TypedNats) -> Result<StateHandle> {
    let mut sub: JetstreamSubscription<WorldStateMessage> = nc.subscribe_jetstream().await?;
    let mut world_state = WorldState::default();

    while sub.has_pending {
        let (message, _) = sub
            .next()
            .await
            .ok_or_else(|| anyhow!("State stream closed before pending messages read."))?;
        world_state.apply(message);
    }

    let state_handle = StateHandle::new(world_state);

    {
        let state_handle = state_handle.clone();
        tokio::spawn(async move {
            while let Some((message, _)) = sub.next().await {
                tracing::info!(?message, "Applying state message");
                state_handle.write_state().apply(message);
            }
        });
    }

    Ok(state_handle)
}

pub async fn update_drone_state(
    nc: &TypedNats,
    message: &DroneStatusMessage,
    time: DateTime<Utc>,
) -> Result<()> {
    nc.publish_jetstream(&WorldStateMessage {
        cluster: message.cluster.clone(),
        message: ClusterStateMessage::DroneMessage(DroneMessage {
            drone: message.drone_id.clone(),
            message: DroneMessageType::State {
                state: message.state.clone(),
            },
        }),
    })
    .await?;

    nc.publish_jetstream(&WorldStateMessage {
        cluster: message.cluster.clone(),
        message: ClusterStateMessage::DroneMessage(DroneMessage {
            drone: message.drone_id.clone(),
            message: DroneMessageType::KeepAlive { timestamp: time },
        }),
    })
    .await?;

    Ok(())
}
