use crate::config::ControllerConfig;
use crate::dns::serve_dns;
use crate::drone_state::monitor_drone_state;
use crate::plan::ControllerPlan;
use crate::run_scheduler;
use anyhow::{Context, Result};
use plane_core::cli::wait_for_interrupt;
use plane_core::messages::agent::BackendStateMessage;
use plane_core::messages::drone_state::UpdateBackendStateMessage;
use plane_core::messages::logging::Component;
use plane_core::messages::state::WorldStateMessage;
use plane_core::nats::TypedNats;
use plane_core::supervisor::Supervisor;
use plane_core::{cli::init_cli, logging::TracingHandle, NeverResult};

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
            .context("UpdateBackendStateMessage subscription ended.")?;
        let value = msg.value.clone();

        let value = BackendStateMessage {
            backend: value.backend,
            state: value.state,
            cluster: value.cluster,
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

/// Heartbeat
pub async fn heartbeat(nc: TypedNats) -> NeverResult {
    loop {
        nc.publish_jetstream(&WorldStateMessage::Heartbeat { heartbeat: None })
            .await?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await
    }
}

async fn controller_main() -> Result<()> {
    let mut tracing_handle = TracingHandle::init(Component::Controller)?;
    let config: ControllerConfig = init_cli()?;
    let plan = ControllerPlan::from_controller_config(config).await?;

    let ControllerPlan {
        nats,
        dns_plan,
        scheduler_plan,
        state,
        http_plan,
    } = plan;

    tracing_handle.attach_nats(nats.clone())?;

    let _scheduler_supervisors = if scheduler_plan.is_some() {
        let scheduler = {
            let nats = nats.clone();
            let state = state.clone();
            Supervisor::new("scheduler", move || {
                run_scheduler(nats.clone(), state.clone())
            })
        };

        let backend_state = {
            let nats = nats.clone();
            Supervisor::new("backend_state", move || {
                update_backend_state_loop(nats.clone())
            })
        };

        let monitor_drone_state = {
            let nats = nats.clone();
            Supervisor::new("monitor_drone_state", move || {
                monitor_drone_state(nats.clone())
            })
        };

        Some([scheduler, backend_state, monitor_drone_state])
    } else {
        None
    };

    #[allow(clippy::manual_map)]
    let _dns_supervisor = if let Some(dns_plan) = dns_plan {
        Some(Supervisor::new("dns", move || serve_dns(dns_plan.clone())))
    } else {
        None
    };

    let _heartbeat_supervisor = {
        let nats = nats.clone();
        Supervisor::new("heartbeat", move || heartbeat(nats.clone()))
    };

    let _http_supervisor = if let Some(http_options) = http_plan {
        let nats = nats.clone();
        let http_options = http_options;
        Some(Supervisor::new("http", move || {
            crate::http_server::serve(http_options.clone(), nats.clone())
        }))
    } else {
        None
    };

    wait_for_interrupt().await
}

pub fn run() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(controller_main())?;

    Ok(())
}
