use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::{
        scheduler::{
            FetchBackendForLock, FetchBackendForLockResponse, ScheduleRequest, ScheduleResponse,
        },
        state::{
            BackendLockMessage, BackendLockMessageType, BackendMessage, BackendMessageType,
            ClusterLockMessage, ClusterLockMessageType, ClusterStateMessage, WorldStateMessage,
        },
    },
    nats::{MessageWithResponseHandle, SubscribeSubject, TypedNats},
    state::StateHandle,
    timing::Timer,
    types::{BackendId, ClusterName, DroneId, PlaneLockState},
    NeverResult,
};
use rand::{thread_rng, Rng};
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
                            bearer_token: spawn_request.bearer_token.clone(),
                        },
                    }),
                })
                .await?;
            if let Some(ref lock) = schedule_request.lock {
                tracing::info!(?lock, ?spawn_request.backend_id, "assigning lock");
                nats.publish_jetstream(&WorldStateMessage {
                    cluster: spawn_request.cluster.clone(),
                    message: ClusterStateMessage::BackendMessage(BackendMessage {
                        backend: spawn_request.backend_id.clone(),
                        message: BackendMessageType::LockMessage(BackendLockMessage {
                            lock: lock.clone(),
                            message: BackendLockMessageType::Assign,
                        }),
                    }),
                })
                .await?;
            }

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

async fn wait_for_lock_assignment(
    state: &mut StateHandle,
    lock: &str,
    cluster_name: &ClusterName,
    nats: &TypedNats,
) -> anyhow::Result<BackendId> {
    let mut sub = nats
        .subscribe_jetstream_subject(SubscribeSubject::<WorldStateMessage>::new(format!(
            "state.cluster.{}.backend.*.lock.{}.assign",
            cluster_name.subject_name(),
            lock
        )))
        .await?;

    let msg = sub.next().await.ok_or_else(|| {
        anyhow!("lock assignment subscription closed before receiving assignment")
    })?;
    tracing::info!(?msg, "received wait for lock assignment");
    let seq = msg.1.sequence;
    state.wait_for_seq(seq).await;
    let WorldStateMessage {
        message : ClusterStateMessage::BackendMessage(
            BackendMessage { backend, .. }), .. } = msg.0 else { panic!() };
    Ok(backend)
}

fn backend_assigned_to_lock(
    state: &StateHandle,
    lock: &str,
    cluster_name: &ClusterName,
) -> anyhow::Result<Option<BackendId>> {
    if let PlaneLockState::Assigned { backend } = state
        .state()
        .cluster(cluster_name)
        .ok_or_else(|| anyhow!("no cluster"))?
        .locked(lock)
    {
        Ok(Some(backend))
    } else {
        Ok(None)
    }
}

async fn announce_lock(
    lock: &str,
    cluster_name: &ClusterName,
    nats: &TypedNats,
    uid: &u64,
) -> anyhow::Result<u64> {
    tracing::info!("announcing lock");
    let seq_id = nats
        .publish_jetstream(&WorldStateMessage {
            cluster: cluster_name.clone(),
            message: ClusterStateMessage::LockMessage(ClusterLockMessage {
                lock: lock.to_string(),
                message: ClusterLockMessageType::Announce { uid: *uid },
            }),
        })
        .await?;

    tracing::info!(?seq_id, "sent announce, will be in state at time");

    Ok(seq_id)
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
    state: &mut StateHandle,
    sr: &ScheduleRequest,
    scheduler: &Scheduler,
    nats: &TypedNats,
) -> anyhow::Result<ScheduleResponse> {
    tracing::info!("checking locks");
    let cluster_name = sr.cluster.clone();
    let schedule_response: Option<_> = if let Some(lock_name) = sr.lock.as_ref() {
        tracing::info!(?lock_name, "scheduling lock");

        if let Ok(Some(backend)) = backend_assigned_to_lock(state, lock_name, &cluster_name) {
            Some(schedule_response_for_existing_backend(
                state,
                cluster_name.clone(),
                backend,
            ))
        } else {
            let my_uid: u64 = {
                let mut rng = thread_rng();
                rng.gen()
            };
            let seq = announce_lock(lock_name, &cluster_name, nats, &my_uid).await?;

            state.wait_for_seq(seq).await;
            let lock_state = state
                .state()
                .cluster(&cluster_name)
                .ok_or_else(|| anyhow!("cluster should exist"))?
                .locked(lock_name);

            match lock_state {
                PlaneLockState::Announced { uid } if uid == my_uid => None,
                PlaneLockState::Announced { .. } => {
                    let assigned_backend =
                        wait_for_lock_assignment(state, lock_name, &cluster_name, nats).await?;
                    Some(schedule_response_for_existing_backend(
                        state,
                        cluster_name.clone(),
                        assigned_backend,
                    ))
                }
                PlaneLockState::Assigned { backend } => {
                    //unlikely, but technically possible
                    Some(schedule_response_for_existing_backend(
                        state,
                        cluster_name.clone(),
                        backend,
                    ))
                }
                _ => None,
            }
        }
    } else {
        None
    };

    if let Some(schedule_response) = schedule_response {
        tracing::info!("Returning existing backend");
        schedule_response
    } else {
        match scheduler.schedule(&cluster_name, Utc::now()) {
            Ok(drone_id) => spawn_backend(nats, &drone_id, &sr.clone()).await,
            Err(SchedulerError::NoDroneAvailable) => Ok(ScheduleResponse::NoDroneAvailable),
        }
    }
}

async fn dispatch_schedule_request(
    mut state: StateHandle,
    schedule_request: MessageWithResponseHandle<ScheduleRequest>,
    scheduler: Scheduler,
    nats: TypedNats,
) {
    let Ok(response) = process_response(
        &mut state,
        &schedule_request.value.clone(),
        &scheduler,
        &nats
    ).await.map_err(|e| tracing::error!("scheduling error {:?}", e)) else {
        tracing::error!(?schedule_request.value, "schedule request failed");
        return;
    };

    tracing::info!("respond to schedule req");

    let Ok(_) = schedule_request.respond(&response).await else {
        tracing::warn!(res = ?response, "schedule response failed to send");
        return;
    };
}

async fn dispatch_lock_request(
    mut state: StateHandle,
    lock_request: MessageWithResponseHandle<FetchBackendForLock>,
    nats: TypedNats,
) {
    let lock_name = &lock_request.value.lock;
    let cluster_name = &lock_request.value.cluster;
    let lock_state = state
        .state()
        .cluster(cluster_name)
        .expect("cluster should exist")
        .locks
        .get(lock_name)
        .expect("lock should exist")
        .clone();

    let response = match lock_state {
        PlaneLockState::Assigned { backend } => {
            let schedule_response = schedule_response_for_existing_backend(
                &state,
                lock_request.value.cluster.clone(),
                backend,
            );

            if let Ok(schedule_response) = schedule_response {
                schedule_response.try_into().expect(
                    "already checked that backend exists, \
				 so schedule_response should be convertible to FetchBackendForLockResponse",
                )
            } else {
                FetchBackendForLockResponse::NoBackendForLock
            }
        }
        PlaneLockState::Announced { .. } => {
            let assigned_backend =
                wait_for_lock_assignment(&mut state, lock_name, cluster_name, &nats).await;

            let schedule_response = assigned_backend.and_then(|assigned_backend| {
                schedule_response_for_existing_backend(
                    &state,
                    lock_request.value.cluster.clone(),
                    assigned_backend,
                )
            });

            if let Ok(schedule_response) = schedule_response {
                schedule_response.try_into().expect(
                    "already checked that backend exists, \
				 so schedule_response should be convertible to FetchBackendForLockResponse",
                )
            } else {
                FetchBackendForLockResponse::NoBackendForLock
            }
        }
        _ => FetchBackendForLockResponse::NoBackendForLock,
    };

    let Ok(_) = lock_request.respond(&response).await else {
        tracing::warn!(res = ?response, "lock response failed to send");
        return;
    };
}

pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state.clone());
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    let mut backend_for_lock_request_sub = nats
        .subscribe(FetchBackendForLock::subscribe_subject())
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
            Some(backend_for_lock_req) = backend_for_lock_request_sub.next() => {
                tracing::info!(metadata=?backend_for_lock_req.value.clone(),
                               "Got locked backend request");
                tokio::spawn(dispatch_lock_request(
                    state.clone(),
                    backend_for_lock_req.clone(),
                    nats.clone(),
                ));
            },
            else => break
        }
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
