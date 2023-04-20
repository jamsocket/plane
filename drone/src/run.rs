use crate::config::DroneConfig;
use crate::{
    agent::run_agent,
    cert::{refresh_if_not_valid, refresh_loop},
    plan::DronePlan,
    proxy::serve,
};
use anyhow::{Context, Result};
use plane_core::cli::init_cli;
use plane_core::logging::TracingHandle;
use plane_core::messages::logging::Component;
use plane_core::retry::do_with_retry;
use plane_core::types::DroneId;
use tokio::signal::unix::SignalKind;

async fn drone_main() -> Result<()> {
    tracing::info!("Starting drone");
    let mut config: DroneConfig = init_cli().context("Initializing CLI")?;

    // Extract drone ID, or generate one if necessary.
    // DronePlan::from_drone_config will do this if we don't do it here,
    // but we want to initialize tracing now, so we ensure the value
    // exists before calling from_drone_config.
    let drone_id = if let Some(drone_id) = &config.drone_id {
        drone_id.clone()
    } else {
        let drone_id = DroneId::new_random();
        config.drone_id = Some(drone_id.clone());
        drone_id
    };
    let mut tracing_handle = TracingHandle::init(Component::Drone { drone_id })
        .context("Initializing tracing handle")?;

    let plan = DronePlan::from_drone_config(config)
        .await
        .context("Constructing drone config")?;
    let DronePlan {
        proxy_options,
        agent_options,
        cert_options,
        nats,
        ..
    } = plan;

    if let Some(nats) = &nats {
        tracing_handle
            .attach_nats(nats.clone())
            .context("Attaching NATS to tracing handle")?;
    }

    if let Some(cert_options) = cert_options {
        do_with_retry(
            || refresh_if_not_valid(&cert_options),
            5,
            std::time::Duration::from_secs(10),
        )
        .await
        .context("Refreshing certificate.")?;

        tokio::spawn(async move {
            loop {
                let cert_options = cert_options.clone();
                let result = refresh_loop(cert_options).await;
                tracing::warn!(?result, "Certificate refresh loop exited.");
            }
        });
    }

    if let Some(proxy_options) = proxy_options {
        tokio::spawn(async move {
            loop {
                let proxy_options = proxy_options.clone();
                let result = serve(proxy_options).await;
                tracing::warn!(?result, "Proxy server exited.");
            }            
        });
    }

    if let Some(agent_options) = agent_options {
        tokio::spawn(async move {
            loop {
                let agent_options = agent_options.clone();
                let result = run_agent(agent_options).await;
                tracing::warn!(?result, "Agent exited.");
            }
        });
        tracing::warn!("Agent exited.");
    }

    let mut int_stream = tokio::signal::unix::signal(SignalKind::interrupt())?;
    let mut term_stream = tokio::signal::unix::signal(SignalKind::terminate())?;
    tokio::select!(
        _ = int_stream.recv() => {
            tracing::info!("Received SIGINT, exiting.");
        },
        _ = term_stream.recv() => {
            tracing::info!("Received SIGTERM, exiting.");
        },
    );

    Ok(())
}

pub fn run() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(drone_main())?;

    Ok(())
}
