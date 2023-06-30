use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::{
        scheduler::{
            FetchLockedBackend, FetchLockedBackendResponse, ScheduleRequest, ScheduleResponse,
        },
        state::{BackendMessage, BackendMessageType, ClusterStateMessage, WorldStateMessage},
    },
    nats::{MessageWithResponseHandle, TypedNats},
    state::StateHandle,
    timing::Timer,
    types::{BackendId, ClusterName, DroneId},
    NeverResult,
};
use scheduler::Scheduler;

mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;

async fn spawn_backend(
    nats: &TypedNats,
    drone: &DroneId,
    schedule_request: &ScheduleRequest,
) -> anyhow::Result<ScheduleResponse> {
    let timer = Timer::new();
    let spawn_request = schedule_request.schedule(drone);
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

            tracing::info!(logical_time=?seq_id, "backend state updated at time");

            Ok(ScheduleResponse::Scheduled {
                drone: drone.clone(),
                backend_id: spawn_request.backend_id,
                bearer_token: spawn_request.bearer_token.clone(),
                spawned: true,
            })
        }
        Ok(false) => {
            tracing::warn!("Drone rejected backend.");
            Ok(ScheduleResponse::NoDroneAvailable)
        }
        Err(error) => {
            tracing::warn!(?error, "Scheduler returned error.");
            Ok(ScheduleResponse::NoDroneAvailable)
        }
    }
}

/// get backend associated with a lock or error
fn backend_of_lock(
    state: &StateHandle,
    cluster_name: &ClusterName,
    lock: &str,
) -> anyhow::Result<BackendId> {
    state
        .state()
        .cluster(cluster_name)
        .ok_or_else(|| anyhow!("no cluster"))?
        .locked(lock)
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

async fn process_response(
    state: &StateHandle,
    sr: &ScheduleRequest,
    scheduler: &Scheduler,
    nats: &TypedNats,
) -> anyhow::Result<ScheduleResponse> {
    tracing::info!("checking locks");
    let cluster_name = sr.cluster.clone();
    if let Some(lock_name) = sr.lock.clone() {
        tracing::info!(?lock_name, "scheduling lock");

        if let Ok(backend) = backend_of_lock(state, &cluster_name, &lock_name) {
            tracing::info!(?backend, "fetch preexisting backend");
            return schedule_response_for_existing_backend(state, cluster_name, backend);
        }

        tracing::info!("spawn with lock");
    }

    match scheduler.schedule(&cluster_name, Utc::now()) {
        Ok(drone_id) => spawn_backend(nats, &drone_id, &sr.clone()).await,
        Err(SchedulerError::NoDroneAvailable) => Ok(ScheduleResponse::NoDroneAvailable),
    }
}

async fn dispatch_schedule_request(
    state: StateHandle,
    schedule_request: MessageWithResponseHandle<ScheduleRequest>,
    scheduler: Scheduler,
    nats: TypedNats,
) {
    let Ok(response) = process_response(
        &state,
        &schedule_request.value.clone(),
        &scheduler,
        &nats
    ).await else {
        tracing::error!(?schedule_request.value, "schedule request failed");
        return;
    };

    let Ok(_) = schedule_request.respond(&response).await else {
        tracing::warn!(res = ?response, "schedule response failed to send");
        return;
    };
}

async fn dispatch_lock_request(
    state: StateHandle,
    lock_request: MessageWithResponseHandle<FetchLockedBackend>,
) {
    let mut response: FetchLockedBackendResponse = FetchLockedBackendResponse::NoBackendForLock;
    if let Ok(backend) = backend_of_lock(&state, &lock_request.value.cluster, &lock_request.value.lock) {
        if let Ok(schedule_response) =
            schedule_response_for_existing_backend(&state, lock_request.value.cluster.clone(), backend)
        {
            response = schedule_response.try_into().unwrap_or(response);
        }
    }
    let Ok(_) = lock_request.respond(&response).await else {
        tracing::warn!(res = ?response, "lock response failed to send");
        return;
    };
}

pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state.clone());
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    let mut locked_backend_request_sub = nats
        .subscribe(FetchLockedBackend::subscribe_subject())
        .await?;

    loop {
        tokio::select! {
            Some(schedule_request) = schedule_request_sub.next() => {
                tracing::info!(metadata=?schedule_request.value.metadata.clone(), "Got spawn request");
                tokio::spawn(dispatch_schedule_request(
                    state.clone(),
                    schedule_request.clone(),
                    scheduler.clone(),
                    nats.clone(),
                ));
            },
            Some(locked_backend_req) = locked_backend_request_sub.next() => {
                tracing::info!(metadata=?locked_backend_req.value.clone(),
                               "Got locked backend request");
                tokio::spawn(dispatch_lock_request(
                    state.clone(),
                    locked_backend_req.clone(),
                ));
            },
            else => break
        }
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
