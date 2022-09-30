use crate::config::DroneConfig;
use crate::{
    agent::run_agent,
    cert::{refresh_if_not_valid, refresh_loop},
    plan::DronePlan,
    proxy::serve,
};
use anyhow::{anyhow, Result};
use dis_spawner::cli::init_cli;
use dis_spawner::logging::TracingHandle;
use dis_spawner::messages::logging::Component;
use dis_spawner::retry::do_with_retry;
use dis_spawner::types::DroneId;
use dis_spawner::NeverResult;
use futures::future::try_join_all;
use futures::Future;
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{pin::Pin, thread};

async fn drone_main() -> NeverResult {
    let mut config: DroneConfig = init_cli()?;

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
    let mut tracing_handle = TracingHandle::init(Component::Drone { drone_id })?;

    let plan = DronePlan::from_drone_config(config).await?;
    let DronePlan {
        proxy_options,
        agent_options,
        cert_options,
        nats,
        ..
    } = plan;

    if let Some(nats) = &nats {
        tracing_handle.attach_nats(nats.clone())?;
    }

    let mut futs: Vec<Pin<Box<dyn Future<Output = NeverResult>>>> = vec![];

    if let Some(cert_options) = cert_options {
        do_with_retry(
            || refresh_if_not_valid(&cert_options),
            5,
            std::time::Duration::from_secs(10),
        )
        .await?;

        futs.push(Box::pin(refresh_loop(cert_options)))
    }

    if let Some(proxy_options) = proxy_options {
        futs.push(Box::pin(serve(proxy_options)));
    }

    if let Some(agent_options) = agent_options {
        futs.push(Box::pin(run_agent(agent_options)))
    }

    try_join_all(futs.into_iter()).await?;
    // try_join_all either returns an Err, or Ok() with a list of Never values.
    // Since Never values are not constructable, if we get here, we can assume that
    // try_join_all returned an empty list. This means that no event loops ever
    // actually ran, i.e. futs was empty.
    Err(anyhow!("No event loops selected."))
}

pub fn run() -> Result<()> {
    let mut signals = Signals::new(&[SIGINT, SIGTERM])?;

    thread::spawn(move || {
        for _ in signals.forever() {
            // TODO: we could shut down containers here.
            std::process::exit(0)
        }
    });

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(drone_main())?;

    Ok(())
}
