use crate::config::ControllerConfig;
use crate::dns::serve_dns;
use crate::plan::ControllerPlan;
use crate::run_scheduler;
use anyhow::{anyhow, Result};
use futures::future::try_join_all;
use plane_core::messages::agent::{BackendStateMessage, UpdateBackendStateMessage};
use plane_core::messages::logging::Component;
use plane_core::nats::TypedNats;
use plane_core::{cli::init_cli, logging::TracingHandle, NeverResult};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::future::Future;
use std::pin::Pin;
use std::thread;

/// Receive UpdateBackendStateMessages over core NATS, and turn them into
/// BackendStateMessages over JetStream.
pub async fn update_backend_state_loop(nc: TypedNats) -> NeverResult {
    let mut sub = nc
        .subscribe(UpdateBackendStateMessage::subscribe_subject())
        .await?;
    loop {
        let msg = sub
            .next()
            .await
            .ok_or_else(|| anyhow!("UpdateBackendStateMessage subscription ended."))?;
        let value = msg.value.clone();

        let value = BackendStateMessage {
            backend: value.backend,
            state: value.state,
            cluster: Some(value.cluster),
            time: value.time,
        };

        if let Err(e) = nc.publish_jetstream(&value).await {
            tracing::error!(error=%e, "Failed to publish backend state message.");
            continue;
        }

        // Ack the message so that the drone doesn't keep retrying.
        msg.respond(&()).await?;
    }
}

async fn controller_main() -> NeverResult {
    let mut tracing_handle = TracingHandle::init(Component::Controller)?;
    let config: ControllerConfig = init_cli()?;
    let plan = ControllerPlan::from_controller_config(config).await?;

    let ControllerPlan {
        nats,
        dns_plan,
        scheduler_plan,
    } = plan;

    tracing_handle.attach_nats(nats.clone())?;

    let mut futs: Vec<Pin<Box<dyn Future<Output = NeverResult>>>> = vec![];

    if scheduler_plan.is_some() {
        futs.push(Box::pin(run_scheduler(nats.clone())));

        futs.push(Box::pin(update_backend_state_loop(nats.clone())));
    }

    if let Some(dns_plan) = dns_plan {
        futs.push(Box::pin(serve_dns(dns_plan)));
    }

    try_join_all(futs.into_iter()).await?;
    // try_join_all either returns an Err, or Ok() with a list of Never values.
    // Since Never values are not constructable, if we get here, we can assume that
    // try_join_all returned an empty list. This means that no event loops ever
    // actually ran, i.e. futs was empty.
    Err(anyhow!("No event loops selected."))
}

pub fn run() -> Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;

    thread::spawn(move || {
        for _ in signals.forever() {
            // TODO: we could shut down containers here.
            std::process::exit(0)
        }
    });

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(controller_main())?;

    Ok(())
}
