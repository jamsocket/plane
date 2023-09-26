use crate::nats::JetStreamable;
use anyhow::anyhow;
use async_nats::jetstream::stream::Config;
pub mod agent;
pub mod cert;
pub mod dns;
pub mod drone_state;
pub mod logging;
pub mod scheduler;
pub mod state;

async fn add_jetstream_stream<T: JetStreamable>(
    jetstream: &async_nats::jetstream::Context,
    config: Option<Config>,
) -> anyhow::Result<()> {
    let config = config.unwrap_or_else(|| T::config());
    tracing::debug!(name = config.name, "Getting or creating jetstream stream.");
    let stream = jetstream
        .get_or_create_stream(config)
        .await
        .map_err(|d| anyhow!("Error: {d:?}"))?;
    tracing::info!(
        name = %stream.cached_info().config.name,
        created_at=%stream.cached_info().created,
        "Got jetstream stream."
    );

    Ok(())
}

pub async fn initialize_jetstreams(
    jetstream: &async_nats::jetstream::Context,
    log_stream_size_limit_bytes: Option<i64>,
) -> anyhow::Result<()> {
    add_jetstream_stream::<state::WorldStateMessage>(jetstream, None).await?;
    let mut log_config = agent::DroneLogMessage::config();
    if let Some(limit_bytes) = log_stream_size_limit_bytes {
        log_config.max_bytes = limit_bytes;
    }
    add_jetstream_stream::<agent::DroneLogMessage>(jetstream, Some(log_config)).await?;
    add_jetstream_stream::<agent::BackendStateMessage>(jetstream, None).await?;
    add_jetstream_stream::<dns::SetDnsRecord>(jetstream, None).await?;

    Ok(())
}
