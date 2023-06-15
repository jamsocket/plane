use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use dashmap::DashMap;
use plane_core::{
    messages::{
        scheduler::{ScheduleRequest, ScheduleResponse},
        state::{BackendMessage, BackendMessageType, ClusterStateMessage, WorldStateMessage},
    },
    nats::{JetStreamable, MessageWithResponseHandle, TypedNats},
    state::StateHandle,
    timing::Timer,
    NeverResult,
};
use scheduler::Scheduler;
use tokio::sync::Notify;

mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;

async fn respond_to_schedule_req(
    scheduler: Scheduler,
    schedule_request: &MessageWithResponseHandle<ScheduleRequest>,
    lock_to_ready: DashMap<String, std::sync::Arc<Notify>>,
    nats: TypedNats,
    state: StateHandle,
) -> anyhow::Result<ScheduleResponse> {
    if let Some(lock) = &schedule_request.value.lock {
        tracing::info!(lock=%lock, "Request includes lock.");

        if let Some(lock_ready_notifier) = lock_to_ready.get(lock) {
            lock_ready_notifier.notified().await;
            lock_to_ready
                .remove(lock)
                .expect("just checked that lock existed in readiness map");
        }

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

                let drone = backend_state
                    .drone
                    .clone()
                    .ok_or_else(|| anyhow!("Lock held by a backend without a drone assignment."))?;

                let bearer_token = backend_state.bearer_token.clone();

                (drone, bearer_token)
            };

            return Ok(ScheduleResponse::Scheduled {
                drone,
                backend_id: backend,
                bearer_token,
                spawned: false,
            });
        }
    }
    match scheduler.schedule(&schedule_request.value.cluster, Utc::now()) {
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

                    let seq_id = nats
                        .publish_jetstream(&WorldStateMessage {
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

                    if let Some(lock) = schedule_request.value.lock.clone() {
                        let notify = std::sync::Arc::new(Notify::new());
                        lock_to_ready.insert(lock, notify.clone());
                        tokio::spawn(async move {
                            while let Ok(current_seq) = nats
                                .get_current_seq_id(WorldStateMessage::stream_name().to_string())
                                .await
                            {
                                if current_seq > seq_id {
                                    break;
                                };
                            }
                            notify.notify_one();
                        });
                    }

                    return Ok(ScheduleResponse::Scheduled {
                        drone: drone_id,
                        backend_id: spawn_request.backend_id,
                        bearer_token: spawn_request.bearer_token.clone(),
                        spawned: true,
                    });
                }
                Ok(false) => {
                    tracing::warn!("Drone rejected backend.");
                    return Ok(ScheduleResponse::NoDroneAvailable);
                }
                Err(error) => {
                    tracing::warn!(?error, "Scheduler returned error.");
                    return Ok(ScheduleResponse::NoDroneAvailable);
                }
            }
        }
        Err(error) => match error {
            SchedulerError::NoDroneAvailable => {
                tracing::warn!("No drone available.");
                return Ok(ScheduleResponse::NoDroneAvailable);
            }
        },
    }
}
pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state.clone());
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");
    let lock_to_ready: DashMap<String, std::sync::Arc<Notify>> = DashMap::new();

    //wrap the whole thing in a func
    while let Some(schedule_request) = schedule_request_sub.next().await {
        tracing::info!(metadata=?schedule_request.value.metadata.clone(), "Got spawn request");

        let nats = nats.clone();
        let schedule_request = schedule_request.clone();
        let state = state.clone();
        let lock_to_ready = lock_to_ready.clone();
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            let Ok(scheduled_response) = respond_to_schedule_req(
				scheduler, &schedule_request, lock_to_ready, nats, state
			).await else {
				tracing::warn!(req = ?schedule_request, "schedule request not handled"); return };
            let Ok(_) = schedule_request.respond(&scheduled_response).await else{
				tracing::warn!(req = ?scheduled_response, "schedule response failed to send"); return};
        });
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
