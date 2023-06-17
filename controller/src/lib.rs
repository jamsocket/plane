use std::collections::HashMap;

use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use dashmap::{mapref::entry::Entry, DashMap};
use futures::lock;
use plane_core::{
    messages::{
        agent::SpawnRequest,
        scheduler::{ScheduleRequest, ScheduleResponse},
        state::{BackendMessage, BackendMessageType, ClusterStateMessage, WorldStateMessage},
    },
    nats::{MessageWithResponseHandle, TypedNats},
    state::{ClusterState, StateHandle},
    timing::Timer,
    types::{BackendId, ClusterName, DroneId},
    NeverResult,
};
use scheduler::Scheduler;
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;

async fn spawn_backend(
    ref state: &StateHandle,
    ref nats: TypedNats,
    drone: DroneId,
    schedule_request: &ScheduleRequest,
) -> anyhow::Result<ScheduleResponse> {
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

            Ok(ScheduleResponse::Scheduled {
                drone,
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

fn locked_backend(
    state: &StateHandle,
    cluster_name: &ClusterName,
    lock: String,
) -> anyhow::Result<BackendId> {
    state
        .state()
        .cluster(cluster_name)
        .ok_or(anyhow!("no cluster"))?
        .locked(&lock)
        .ok_or(anyhow!("no backend"))
}

fn fetch_backend(
    state: &StateHandle,
    cluster: ClusterName,
    backend: BackendId,
) -> anyhow::Result<ScheduleResponse> {
    // Anything that fails to find the drone results in an error here, since we just
    // checked that the lock is held which implies that the drone exists.
    let state = state.state();
    let Some(cluster) = state.cluster(&cluster) else {
		panic!()
	};

    let (drone, bearer_token) = {
        let backend_state = cluster.backend(&backend).unwrap();
        //.ok_or_else(|| anyhow!("Lock held by a backend that doesn't exist."))?;

        let drone = backend_state.drone.clone().unwrap();
        //.ok_or_else(|| anyhow!("Lock held by a backend without a drone assignment."))?;

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

/*
async fn respond_to_schedule_req(
    scheduler: Scheduler,
    schedule_request: &MessageWithResponseHandle<ScheduleRequest>,
    lock_to_ready: Arc<DashMap<String, Mutex<Option<u64>>>>,
    //lock_to_ready: Option<Arc<Mutex<u64>>>,
    nats: TypedNats,
    ref state: StateHandle,
) -> anyhow::Result<ScheduleResponse> {
    let lockguard = if let Some(lock) = &schedule_request.value.lock {
        tracing::info!(lock=%lock, "Request includes lock.");

        let entry = lock_to_ready.entry(lock.to_string()).or_insert(Mutex::new(None));

        Some(entry)
    } else { None };

    if let Some(lockguard) = lockguard {
        let lock = lockguard.lock().await;
        if lock.is_some() {
            let ln = state.state().get_listener(lock.unwrap());

            if let Ok(mut ln) = ln {
                ln.notified().await;
            }}

        if let Ok(res) = fetch_locked_backend(state, &schedule_request.value) {
            return Ok(res);
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

                    if let Some(lockguard) = lock_to_ready.get(lock.to_string()) {
                        let mut sup = lockguard.lock().await;
                        *sup = Some(seq_id);
                    }

                    Ok(ScheduleResponse::Scheduled {
                        drone: drone_id,
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
        Err(error) => match error {
            SchedulerError::NoDroneAvailable => {
                tracing::warn!("No drone available.");
                Ok(ScheduleResponse::NoDroneAvailable)
            }
        },
    }
}
*/

async fn dispatch(
	state: StateHandle,
	cluster_name: ClusterName,
	sr: ScheduleRequest,
	scheduler: Scheduler,
	nats: TypedNats,
	lock: Option<String>,
	lock_to_ready: Arc<DashMap<String, Mutex<Option<u64>>>>
	) -> anyhow::Result<ScheduleResponse> {
	tracing::info!("checking locks");
	if let Some(lock) = lock {
		let bk = lock_to_ready.entry(lock.clone()).or_insert(Mutex::new(None));
		tracing::info!("If you can't see this, dashmap has deadlocked");
		let _lg = bk.lock().await;

		tracing::info!("claiming lock mutex");
		if let Ok(backend) = locked_backend(&state, &cluster_name, lock) {
			tracing::info!("fetch preexisting backend");
			return fetch_backend(&state, cluster_name, backend)
		} 
		tracing::info!("spawn with lock");
		let drone = scheduler.schedule(&cluster_name, Utc::now()).unwrap();
		spawn_backend(&state, nats.clone(), drone, &sr.clone()).await
	} else {
		let drone = scheduler.schedule(&cluster_name, Utc::now()).unwrap();
		spawn_backend(&state, nats.clone(), drone, &sr.clone()).await
	}
}

pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state.clone());
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");
    let lock_to_ready: std::sync::Arc<DashMap<String, Mutex<Option<u64>>>> =
        std::sync::Arc::new(DashMap::new());

    //wrap the whole thing in a func
    while let Some(schedule_request) = schedule_request_sub.next().await {
        tracing::info!(metadata=?schedule_request.value.metadata.clone(), "Got spawn request");

        let nats = nats.clone();
        let schedule_request = schedule_request.clone();
        let state = state.clone();
        let lock_to_ready = lock_to_ready.clone();
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            let sr = schedule_request.value.clone();
            let lock = sr.lock.clone();
            tracing::info!(?lock, "scheduling lock");
            let cluster_name = sr.cluster.clone();

            //let Ok(response) = if let Some((cs, backend)) = lock
            let Ok(response) = dispatch(
					state.clone(),
					cluster_name,
					sr,
					scheduler.clone(),
					nats.clone(),
					lock.clone(),
					lock_to_ready.clone()
			).await else { panic!() };
			tracing::info!("all locks should have been dropped!");
				/*
				if let Some(lock) = lock {
					let bk = lock_to_ready.entry(lock.clone()).or_insert(Mutex::new(None));
					let lg = bk.lock().await;
				}
				if let Some(Ok(backend)) = lock.map(
				|lock| {
					lock_to_ready.entry(lock.clone()).or_insert(Mutex::new(None));
					locked_backend(&state, &cluster_name, lock)
				}) {
				tracing::info!("fetching existing backend");
				fetch_backend(&state, cluster_name, backend)
			} else {
				tracing::info!("spooling up new backend");
				let drone = scheduler.schedule(&cluster_name, Utc::now())
					.unwrap();
				spawn_backend(&state, nats.clone(), drone, &sr.clone()).await
				}}) else { panic!() };
            //} else { panic!() };*/
            /*
            let Ok(scheduled_response) = respond_to_schedule_req(
                scheduler, &schedule_request, lock_to_ready, nats, state
            ).await else {
                tracing::warn!(req = ?schedule_request, "schedule request not handled"); return };
            */
            let Ok(_) = schedule_request.respond(&response).await else {
				tracing::warn!(req = ?response, "schedule response failed to send"); return
			};
        });
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
