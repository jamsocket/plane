use crate::{nats::JetStreamable, util::LogAndIgnoreError};
use anyhow::anyhow;
pub mod agent;
pub mod cert;
pub mod dns;
pub mod drone_state;
pub mod logging;
pub mod scheduler;
pub mod state;

async fn add_jetstream_stream<T: JetStreamable>(
    jetstream: &async_nats::jetstream::Context,
) -> anyhow::Result<()> {
    let config = T::config();
    tracing::debug!(name = config.name, "Getting or creating jetstream stream.");
    let result = jetstream.get_or_create_stream(config).await;

    let stream = if let Err(e) = result {
        if e.to_string()
            .contains("replicas > 1 not supported in non-clustered mode")
        {
            // If we are running in non-clustered mode (e.g. in dev or testing),
            // we can't use replicas, so we overwrite the config to disable them.

            let config = async_nats::jetstream::stream::Config {
                num_replicas: 1,
                ..T::config()
            };

            let result = jetstream.get_or_create_stream(config).await;

            result.map_err(|e| anyhow!("Error: {e:?}"))?
        } else {
            return Err(anyhow!("Error: {e:?}"));
        }
    } else {
        result.map_err(|e| anyhow!("Error: {e:?}"))?
    };

    tracing::info!(
        name = %stream.cached_info().config.name,
        created_at=%stream.cached_info().created,
        "Got jetstream stream."
    );

    Ok(())
}

pub async fn initialize_jetstreams(
    jetstream: &async_nats::jetstream::Context,
) -> anyhow::Result<()> {
    add_jetstream_stream::<state::WorldStateMessage>(jetstream)
        .await
        .log_and_ignore_error();
    add_jetstream_stream::<agent::BackendLogMessage>(jetstream)
        .await
        .log_and_ignore_error();
    add_jetstream_stream::<agent::BackendStateMessage>(jetstream)
        .await
        .log_and_ignore_error();
    add_jetstream_stream::<dns::SetDnsRecord>(jetstream)
        .await
        .log_and_ignore_error();

    Ok(())
}
