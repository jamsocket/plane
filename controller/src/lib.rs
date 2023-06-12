use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::{
        scheduler::{ScheduleRequest, ScheduleResponse},
        state::{BackendMessage, BackendMessageType, ClusterStateMessage, WorldStateMessage},
    },
    nats::TypedNats,
    state::StateHandle,
    timing::Timer,
    NeverResult,
};
use scheduler::Scheduler;

mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;

pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state.clone());
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    while let Some(schedule_request) = schedule_request_sub.next().await {
        tracing::info!(metadata=?schedule_request.value.metadata.clone(), "Got spawn request");

        if let Some(lock) = &schedule_request.value.lock {
            tracing::info!(lock=%lock, "Request includes lock.");
            let locked = {
                let state = state.state();
                let cluster = state
                    .cluster(&schedule_request.value.cluster)
                    .ok_or_else(|| anyhow!("Cluster does not exist."))?;
                cluster.locked(lock)
            };
            tracing::info!(lock=%lock, ?locked, "Lock checked.");

            if let Some(backend) = locked {
                tracing::info!(lock=%lock, "Lock is held.");

                // Anything that fails to find the drone results in an error here, since we just
                // checked that the lock is held which implies that the drone exists.

                let (drone, bearer_token) = {
                    let state = state.state();
                    let cluster = state
                        .cluster(&schedule_request.value.cluster)
                        .ok_or_else(|| anyhow!("Expected cluster to exist."))?;

                    let backend_state = cluster
                        .backend(&backend)
                        .ok_or_else(|| anyhow!("Lock held by a backend that doesn't exist."))?;

                    let drone = backend_state.drone.clone().ok_or_else(|| {
                        anyhow!("Lock held by a backend without a drone assignment.")
                    })?;

                    let bearer_token = backend_state.bearer_token.clone();

                    (drone, bearer_token)
                };

                schedule_request
                    .respond(&ScheduleResponse::Scheduled {
                        drone,
                        backend_id: backend,
                        bearer_token,
                        spawned: false,
                    })
                    .await?;
                continue;
            }
        }

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

                        nats.publish_jetstream(&WorldStateMessage {
                            cluster: schedule_request.value.cluster.clone(),
                            message: ClusterStateMessage::BackendMessage(BackendMessage {
                                backend: spawn_request.backend_id.clone(),
                                message: BackendMessageType::Assignment {
                                    drone: drone_id.clone(),
                                    lock: schedule_request.value.lock.clone(),
                                    bearer_token: spawn_request.bearer_token.clone(),
                                },
                            }),
                        })
                        .await?;

                        ScheduleResponse::Scheduled {
                            drone: drone_id,
                            backend_id: spawn_request.backend_id,
                            bearer_token: spawn_request.bearer_token.clone(),
                            spawned: true,
                        }
                    }
                    Ok(false) => {
                        tracing::warn!("Drone rejected backend.");
                        ScheduleResponse::NoDroneAvailable
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Scheduler returned error.");
                        ScheduleResponse::NoDroneAvailable
                    }
                }
            }
            Err(error) => match error {
                SchedulerError::NoDroneAvailable => {
                    tracing::warn!("No drone available.");
                    ScheduleResponse::NoDroneAvailable
                }
            },
        };

        schedule_request.respond(&result).await?;
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
