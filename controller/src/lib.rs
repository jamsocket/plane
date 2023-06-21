use std::collections::HashMap;

use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::{
        scheduler::{ScheduleRequest, ScheduleResponse},
        state::{BackendMessage, BackendMessageType, ClusterStateMessage, WorldStateMessage},
    },
    nats::TypedNats,
    state::{ClosableNotify, SequenceNumberInThePast, StateHandle},
    timing::Timer,
    types::{BackendId, ClusterName, DroneId},
    NeverResult,
};
use scheduler::Scheduler;
use std::sync::Arc;
use tokio::sync::Mutex;

mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;

async fn spawn_backend(
    ref nats: TypedNats,
    drone: DroneId,
    schedule_request: &ScheduleRequest,
) -> anyhow::Result<(ScheduleResponse, Option<u64>)> {
    let timer = Timer::new();
    let spawn_request = schedule_request.schedule(&drone);
    match nats.request(&spawn_request).await {
        Ok(true) => {
            tracing::info!(
                duration=?timer.duration(),
                backend_id=%spawn_request.backend_id,
                %drone,
                "Drone accepted backend."
            );

            let seq_id = nats
                .publish_jetstream(&WorldStateMessage {
                    cluster: schedule_request.cluster.clone(),
                    message: ClusterStateMessage::BackendMessage(BackendMessage {
                        backend: spawn_request.backend_id.clone(),
                        message: BackendMessageType::Assignment {
                            drone: drone.clone(),
                            lock: schedule_request.lock.clone(),
                            bearer_token: spawn_request.bearer_token.clone(),
                        },
                    }),
                })
                .await?;

            tracing::info!(
            logical_time=?seq_id,
            "backend state updated at time"
                );

            Ok((
                ScheduleResponse::Scheduled {
                    drone,
                    backend_id: spawn_request.backend_id,
                    bearer_token: spawn_request.bearer_token.clone(),
                    spawned: true,
                },
                Some(seq_id),
            ))
        }
        Ok(false) => {
            tracing::warn!("Drone rejected backend.");
            Ok((ScheduleResponse::NoDroneAvailable, None))
        }
        Err(error) => {
            tracing::warn!(?error, "Scheduler returned error.");
            Ok((ScheduleResponse::NoDroneAvailable, None))
        }
    }
}

fn locked_backend(
    state: &StateHandle,
    cluster_name: &ClusterName,
    lock: String,
) -> anyhow::Result<BackendId> {
    state
        .state()
        .cluster(cluster_name)
        .ok_or_else(|| anyhow!("no cluster"))?
        .locked(&lock)
        .ok_or_else(|| anyhow!("no backend"))
}

fn schedule_response_for_existing_backend(
    state: &StateHandle,
    cluster: ClusterName,
    backend: BackendId,
) -> anyhow::Result<ScheduleResponse> {
    // Anything that fails to find the drone results in an error here, since we just
    // checked that the lock is held which implies that the drone exists.
    let state = state.state();
    tracing::info!("getting cluster from state");
    let cluster = state
        .cluster(&cluster)
        .ok_or_else(|| anyhow!("no such cluster"))?;
    tracing::info!("fetching backend!");
    let (drone, bearer_token) = {
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

    Ok(ScheduleResponse::Scheduled {
        drone,
        backend_id: backend,
        bearer_token,
        spawned: false,
    })
}

async fn dispatch(
    state: StateHandle,
    cluster_name: ClusterName,
    sr: ScheduleRequest,
    scheduler: Scheduler,
    nats: TypedNats,
    locker: Option<(String, Arc<Mutex<Option<ClosableNotify>>>)>,
) -> anyhow::Result<ScheduleResponse> {
    tracing::info!("checking locks");
    if let Some((lock_name, lock)) = locker {
        tracing::info!(?lock, "locking mutex for lock");
        let mut l = lock.lock().await;
        if l.is_some() {
            l.as_mut().unwrap().notified().await;
        }

        if let Ok(backend) = locked_backend(&state, &cluster_name, lock_name.clone()) {
            tracing::info!(?backend, "fetch preexisting backend");
            return schedule_response_for_existing_backend(&state, cluster_name, backend);
        }

        tracing::info!("spawn with lock");
        let drone = match scheduler.schedule(&cluster_name, Utc::now()) {
            Ok(drone_id) => drone_id,
            Err(SchedulerError::NoDroneAvailable) => return Ok(ScheduleResponse::NoDroneAvailable),
        };
        match spawn_backend(nats.clone(), drone, &sr.clone()).await? {
            (res, Some(st)) => {
                match state.state().get_listener(st) {
                    Ok(listener) => {
                        *l = Some(listener);
                    }
                    Err(SequenceNumberInThePast) => {
                        tracing::warn!("tried to insert notifier after valid time");
                    }
                };
                Ok(res)
            }
            (res, None) => Ok(res),
        }
    } else {
        match scheduler.schedule(&cluster_name, Utc::now()) {
            Ok(drone_id) => Ok(spawn_backend(nats.clone(), drone_id, &sr.clone()).await?.0),
            Err(SchedulerError::NoDroneAvailable) => Ok(ScheduleResponse::NoDroneAvailable),
        }
    }
}

type WaitMap = HashMap<String, Arc<Mutex<Option<ClosableNotify>>>>;
pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state.clone());
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");
    let mut lock_to_ready: WaitMap = HashMap::new();

    while let Some(schedule_request) = schedule_request_sub.next().await {
        tracing::info!(metadata=?schedule_request.value.metadata.clone(), "Got spawn request");

        let nats = nats.clone();
        let schedule_request = schedule_request.clone();
        let state = state.clone();
        let lock = schedule_request.value.lock.clone();
        let lock_to_ready = &mut lock_to_ready;
        let lock_mut_maybe = lock.clone().map(|lock| {
            lock_to_ready
                .entry(lock)
                .or_insert(Arc::new(Mutex::new(None)))
                .clone()
        });
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            let sr = schedule_request.value.clone();
            tracing::info!(?lock, "scheduling lock");
            let cluster_name = sr.cluster.clone();

            let Ok(response) = dispatch(
                state.clone(),
                cluster_name,
                sr.clone(),
                scheduler.clone(),
                nats.clone(),
                sr.lock.clone().zip(lock_mut_maybe)
            ).await else {
				tracing::error!(?sr, "schedule request failed");
				panic!("failed to dispatch");
			};
            tracing::info!(?response, "the response");

            let Ok(_) = schedule_request.respond(&response).await else {
				tracing::warn!(req = ?response, "schedule response failed to send");
				panic!("failed to respond to schedule req");
			};
        });
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
