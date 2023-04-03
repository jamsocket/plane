use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::{
        drone_state::DroneStatusMessage,
        scheduler::{ScheduleRequest, ScheduleResponse},
    },
    nats::TypedNats,
    timing::Timer,
    NeverResult,
};
use scheduler::Scheduler;
use state::StateHandle;
use tokio::select;

use crate::scheduler::SchedulerError;

pub mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;
pub mod state;
pub mod ttl_store;

pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(nats.clone(), state);
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    let mut status_sub = nats
        .subscribe(DroneStatusMessage::subscribe_subject())
        .await?;
    tracing::info!("Subscribed to drone status messages.");

    loop {
        select! {
            status_msg = status_sub.next() => {
                if let Some(status_msg) = status_msg {
                    tracing::debug!(status_msg=?status_msg.value, "Got drone status");
                    let time = Utc::now();

                    scheduler.update_status(time, &status_msg.value).await?;
                } else {
                    return Err(anyhow!("status_sub.next() returned None."));
                }
            },

            schedule_request = schedule_request_sub.next() => {
                match schedule_request {
                    Some(schedule_request) => {
                        tracing::info!(spawn_request=?schedule_request.value, "Got spawn request");
                        let result = match scheduler.schedule(&schedule_request.value.cluster, Utc::now()) {
                            Ok(drone_id) => {
                                let timer = Timer::new();
                                let spawn_request = schedule_request.value.schedule(&drone_id);
                                match nats.request(&spawn_request).await {
                                    Ok(true) => {
                                        tracing::info!(
                                            duration=?timer.duration(),
                                            backend_id=%spawn_request.backend_id,
                                            %drone_id,
                                            "Drone accepted backend."
                                        );
                                        ScheduleResponse::Scheduled {
                                            drone: drone_id,
                                            backend_id: spawn_request.backend_id,
                                            bearer_token: spawn_request.bearer_token,
                                        }
                                    }
                                    Ok(false) => {
                                        tracing::warn!("No drone available.");
                                        ScheduleResponse::NoDroneAvailable
                                    },
                                    Err(error) => {
                                        tracing::warn!(?error, "Scheduler returned error.");
                                        ScheduleResponse::NoDroneAvailable
                                    },
                                }
                            },
                            Err(error) => {
                                match error {
                                    SchedulerError::NoDroneAvailable => {
                                        tracing::warn!("No drone available.");
                                        ScheduleResponse::NoDroneAvailable
                                    },
                                }
                            },
                        };

                        schedule_request.respond(&result).await?;
                    },
                    None => return Err(anyhow!("spawn_request_sub.next() returned None.")),
                }
            }
        }
    }
}
