use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::agent::DroneStatusMessage,
    messages::scheduler::{ScheduleRequest, ScheduleResponse},
    nats::TypedNats,
    NeverResult,
};
use scheduler::Scheduler;
use tokio::select;

mod config;
mod dns;
mod plan;
pub mod run;
mod scheduler;
pub mod ttl_store;

pub async fn run_scheduler(nats: TypedNats) -> NeverResult {
    let scheduler = Scheduler::default();
    let mut spawn_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    let mut status_sub = nats
        .subscribe_jetstream(DroneStatusMessage::subscribe_subject())
        .await?;
    tracing::info!("Subscribed to drone status messages.");

    loop {
        select! {
            status_msg = status_sub.next() => {
                tracing::debug!(?status_msg, "Got drone status");
                if let Some(status_msg) = status_msg? {
                    scheduler.update_status(Utc::now(), &status_msg);
                } else {
                    return Err(anyhow!("status_sub.next() returned None."));
                }
            },

            spawn_request = spawn_request_sub.next() => {
                match spawn_request {
                    Ok(Some(schedule_request)) => {
                        tracing::info!(spawn_request=?schedule_request.value, "Got spawn request");
                        let result = match scheduler.schedule(&schedule_request.value.cluster, Utc::now()) {
                            Ok(drone_id) => {
                                let spawn_request = schedule_request.value.schedule(&drone_id);
                                match nats.request(&spawn_request).await {
                                    Ok(false) => {
                                        tracing::warn!("No drone available.");
                                        ScheduleResponse::NoDroneAvailable
                                    },
                                    Err(error) => {
                                        tracing::warn!(?error, "Scheduler returned error.");
                                        ScheduleResponse::NoDroneAvailable
                                    },
                                    Ok(true) => ScheduleResponse::Scheduled { drone: drone_id, backend_id: spawn_request.router.backend_id }
                                }
                            },
                            Err(error) => {
                                tracing::warn!(?error, "Communication error during scheduling.");
                                ScheduleResponse::NoDroneAvailable
                            },
                        };

                        schedule_request.respond(&result).await?;
                    },
                    Ok(None) => return Err(anyhow!("spawn_request_sub.next() returned None.")),
                    Err(err) => tracing::warn!("spawn_request_sub.next() returned error: {:?}", err)
                }
            }
        }
    }
}
