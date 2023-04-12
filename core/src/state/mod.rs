use crate::{
    messages::state::WorldStateMessage,
    nats::{JetstreamSubscription, TypedNats},
};

pub use self::world_state::{StateHandle, WorldState};
use anyhow::{anyhow, Result};

mod world_state;

async fn get_world_state_from_sub(
    sub: &mut JetstreamSubscription<WorldStateMessage>,
) -> Result<WorldState> {
    let mut world_state = WorldState::default();

    while sub.has_pending {
        let (message, _) = sub
            .next()
            .await
            .ok_or_else(|| anyhow!("State stream closed before pending messages read."))?;
        world_state.apply(message);
    }

    Ok(world_state)
}

pub async fn get_world_state(nc: TypedNats) -> Result<WorldState> {
    let mut sub: JetstreamSubscription<WorldStateMessage> = nc.subscribe_jetstream().await?;
    get_world_state_from_sub(&mut sub).await
}

/// Start a loop which reads the state stream and applies the messages to the world state.
/// Returns a handle to the world state.
pub async fn start_state_loop(nc: TypedNats) -> Result<StateHandle> {
    let mut sub: JetstreamSubscription<WorldStateMessage> = nc.subscribe_jetstream().await?;
    let world_state = get_world_state_from_sub(&mut sub).await?;

    let state_handle = StateHandle::new(world_state);

    {
        let state_handle = state_handle.clone();
        tokio::spawn(async move {
            while let Some((message, _)) = sub.next().await {
                state_handle.write_state().apply(message);
            }
        });
    }

    Ok(state_handle)
}
