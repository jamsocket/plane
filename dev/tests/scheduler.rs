use anyhow::Result;
use chrono::Utc;
use integration_test::integration_test;
use plane_controller::{
    drone_state::monitor_drone_state, run::update_backend_state_loop, run_scheduler,
};
use plane_core::{
    messages::{
        agent::{BackendState, BackendStateMessage, DroneState, SpawnRequest},
        drone_state::{DroneConnectRequest, DroneStatusMessage, UpdateBackendStateMessage},
        scheduler::{FetchBackendForLock, FetchBackendForLockResponse, ScheduleResponse},
    },
    nats::TypedNats,
    state::{start_state_loop, StateHandle},
    types::{BackendId, ClusterName, DroneId, ResourceLock},
    NeverResult,
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, timeout, LivenessGuard},
    util::{base_scheduler_request, random_loopback_ip, wait_for_predicate},
};
use std::net::IpAddr;

pub const CLUSTER_DOMAIN: &str = "plane.test";
const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct MockAgent {
    nats: TypedNats,
    cluster: ClusterName,
    state: StateHandle,
    _schedule_req_monitor: LivenessGuard<()>,
    _state_monitor: LivenessGuard<NeverResult>,
}

async fn accept_spawn_reqs(nats: TypedNats, drone_id: DroneId, cluster: ClusterName) {
    let mut sub = nats
        .subscribe(SpawnRequest::subscribe_subject(&cluster, &drone_id))
        .await
        .unwrap();
    while let Some(spawn_msg) = sub.next().await {
        tracing::info!(?spawn_msg, "received spawn msg");
        assert_eq!(
            drone_id, spawn_msg.value.drone_id,
            "Scheduled drone did not match expectations"
        );

        spawn_msg.respond(&true).await.unwrap();
    }
}

impl MockAgent {
    pub async fn new(nats: TypedNats, drone_id: &DroneId, state: StateHandle) -> Self {
        let ip: IpAddr = random_loopback_ip().into();
        let cluster = ClusterName::new(CLUSTER_DOMAIN);
        let request = DroneConnectRequest {
            drone_id: drone_id.clone(),
            cluster: cluster.clone(),
            ip,
            version: Some("0.1.0".to_string()),
            git_hash: None,
        };

        let state_monitor = expect_to_stay_alive(monitor_drone_state(nats.clone()));
        let schedule_req_monitor = expect_to_stay_alive(accept_spawn_reqs(
            nats.clone(),
            drone_id.clone(),
            cluster.clone(),
        ));

        let drone_copy = drone_id.clone();
        let cluster_clone = cluster.clone();
        let drone_connected = wait_for_predicate(state.clone(), move |ws| {
            tracing::info!(?ws, "current ws");
            let Some(cluster) = ws.cluster(&cluster_clone) else {
                return false;
            };
            tracing::info!(?cluster, "cluster");
            let Some(drone) = cluster.drone(&drone_copy) else {
                return false;
            };
            drone.meta.is_some()
        });

        // this waits for the state machine to initialize.
        // which also implies the controller is ready for requests.
        wait_for_predicate(state.clone(), |_| true).await;

        let result = nats.request(&request).await.unwrap();
        drone_connected.await;

        assert!(result, "Drone connect request should succeed.");

        MockAgent {
            nats,
            cluster,
            state,
            _schedule_req_monitor: schedule_req_monitor,
            _state_monitor: state_monitor,
        }
    }

    /// This provides a way to spawn backends without needing different function calls
    /// for spawning vs retrieval.
    pub async fn schedule_drone(
        &self,
        request_bearer_token: bool,
        lock: Option<ResourceLock>,
    ) -> Result<ScheduleResponse> {
        let cluster = &self.cluster;
        let mut request = base_scheduler_request();
        request.require_bearer_token = request_bearer_token;
        request.lock = lock.clone();

        let locked_backend = lock
            .clone()
            .map(|lock| self.state.state().cluster(cluster).unwrap().locked(&lock));

        let response = self
            .nats
            .request_with_timeout(&request, std::time::Duration::from_secs(30))
            .await;

        if let Ok(ScheduleResponse::Scheduled {
            backend_id,
            spawned,
            ..
        }) = &response
        {
            if let Some(plane_core::types::LockState::Assigned { backend }) = locked_backend {
                assert!(!(*spawned));
                assert_eq!(*backend_id, backend);
            }

            let backend_id_copy = backend_id.clone();
            let cluster_copy = cluster.clone();

            // it's technically possible for the state to not
            // incorporate the backend by the time the response
            // returns, so we await it.
            wait_for_predicate(self.state.clone(), move |ws| {
                let backend_id = &backend_id_copy;
                let cluster = &cluster_copy;
                ws.cluster(cluster)
                    .and_then(|cluster| cluster.backends.get(backend_id))
                    .is_some()
            })
            .await;

            let state = self.state.state();
            let backend = state
                .cluster(cluster)
                .expect("Cluster should exist.")
                .backends
                .get(backend_id)
                .expect("Backend should exist.");

            assert!(self
                .state
                .get_ready_drones(cluster, Utc::now())
                .unwrap()
                .contains(backend.drone.as_ref().unwrap()));
        }

        tracing::info!(?response, ?lock, "schedule_drone lock");

        response
    }

    pub async fn fetch_locked(
        &self,
        cluster: ClusterName,
        lock: plane_core::types::ResourceLock,
    ) -> Result<FetchBackendForLockResponse> {
        let req = FetchBackendForLock { cluster, lock };

        let res = self.nats.request(&req).await?;

        Ok(res)
    }
}

async fn drone_ready_notify(state: StateHandle, drone: DroneId, cluster: ClusterName) {
    wait_for_predicate(state, move |ws| {
        tracing::info!(?ws, "current ws");
        let Some(cluster) = ws.cluster(&cluster) else {
            return false;
        };
        tracing::info!(?cluster, "cluster");
        let Some(drone) = cluster.drone(&drone) else {
            return false;
        };
        tracing::info!(?drone, "drone");
        let Some(drone_state) = drone.state() else {
            return false;
        };
        tracing::info!(?drone_state, "drone state");
        drone_state == DroneState::Ready && drone.last_seen.is_some()
    })
    .await
}

async fn drone_not_ready_notify(state: StateHandle, drone: DroneId, cluster: ClusterName) {
    wait_for_predicate(state, move |ws| {
        tracing::info!(?ws, "current ws");
        let Some(cluster) = ws.cluster(&cluster) else {
            return false;
        };
        tracing::info!(?cluster, "cluster");
        let Some(drone) = cluster.drone(&drone) else {
            return false;
        };
        tracing::info!(?drone, "drone");
        let Some(drone_state) = drone.state() else {
            return false;
        };
        tracing::info!(?drone_state, "drone state");
        drone_state != DroneState::Ready && drone.last_seen.is_some()
    })
    .await
}

#[integration_test]
async fn no_drone_available() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));

    let request = base_scheduler_request();
    tracing::info!("Making spawn request.");
    let result = timeout(
        1_000,
        "Schedule request should be responded.",
        tokio::spawn(async move { nats_conn.request(&request).await }),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();

    assert_eq!(ScheduleResponse::NoDroneAvailable, result);
}

#[integration_test]
async fn one_drone_available() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();

    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));

    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;

    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;
    let result = mock_agent.schedule_drone(false, None).await.unwrap();

    assert!(matches!(&result, ScheduleResponse::Scheduled { drone, .. } if drone == &drone_id));
    let backend_id = match result {
        ScheduleResponse::Scheduled { backend_id, .. } => backend_id,
        _ => panic!("Unexpected schedule response."),
    };

    let ws = state.state();
    let assigned_drone = ws
        .cluster(&ClusterName::new(CLUSTER_DOMAIN))
        .unwrap()
        .backends
        .get(&backend_id)
        .unwrap()
        .drone
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(&drone_id.clone(), &assigned_drone);
}

#[integration_test]
async fn drone_not_ready() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let drone_id = DroneId::new_random();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));

    let drone_not_ready = drone_not_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: false,
            state: DroneState::Draining,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_not_ready.await;

    let result = mock_agent.schedule_drone(false, None).await.unwrap();

    assert_eq!(ScheduleResponse::NoDroneAvailable, result);
}

#[integration_test]
async fn drone_becomes_not_ready() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let drone_id = DroneId::new_random();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));

    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;

    let drone_not_ready = drone_not_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: false,
            state: DroneState::Draining,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_not_ready.await;

    let result = mock_agent.schedule_drone(false, None).await.unwrap();

    assert_eq!(ScheduleResponse::NoDroneAvailable, result);
}

#[integration_test]
async fn schedule_request_bearer_token() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;

    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;

    let result = mock_agent.schedule_drone(true, None).await.unwrap();

    if let ScheduleResponse::Scheduled {
        drone,
        bearer_token,
        ..
    } = result
    {
        assert_eq!(drone, drone_id);

        if let Some(bearer_token) = bearer_token {
            assert_eq!(30, bearer_token.len());
        } else {
            panic!("Bearer token should be present");
        }
    } else {
        panic!("Expected ScheduleResponse::Scheduled, got {:?}", result);
    }
}

#[integration_test]
async fn test_update_backend_stats_message() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let _update_backend_guard = expect_to_stay_alive(update_backend_state_loop(nats_conn.clone()));
    let backend_id = BackendId::new_random();
    let drone_id = DroneId::new_random();
    let time = Utc::now();

    let mut sub = nats_conn
        .subscribe_jetstream_subject(BackendStateMessage::subscribe_subject(&backend_id))
        .await
        .unwrap();

    nats_conn
        .request(&UpdateBackendStateMessage {
            backend: backend_id.clone(),
            state: BackendState::Ready,
            time,
            cluster: ClusterName::new("plane.test"),
            drone: drone_id.clone(),
        })
        .await
        .unwrap();

    let result = timeout(10_000, "Did not receive BackendStateMessage", sub.next())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        BackendStateMessage {
            backend: backend_id,
            state: BackendState::Ready,
            time,
            cluster: ClusterName::new("plane.test"),
        },
        result.0
    );
}

#[integration_test]
async fn schedule_request_lock() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;

    let foobar: ResourceLock = "foobar".to_string().try_into().unwrap();
    let r1 = mock_agent
        .schedule_drone(false, Some(foobar.clone()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled {
        drone: drone1,
        backend_id: backend1,
        ..
    } = r1
    else {
        panic!()
    };

    let r2 = mock_agent
        .schedule_drone(false, Some(foobar))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled {
        drone: drone2,
        backend_id: backend2,
        ..
    } = r2
    else {
        panic!()
    };

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);

    let speedlock: ResourceLock = "speedlock".to_string().try_into().unwrap();

    //testing simultaneous schedule requests to same drone
    match tokio::join!(
        mock_agent.schedule_drone(false, Some(speedlock.clone())),
        mock_agent.schedule_drone(false, Some(speedlock))
    ) {
        (
            Ok(ScheduleResponse::Scheduled { spawned: a, .. }),
            Ok(ScheduleResponse::Scheduled { spawned: b, .. }),
        ) => {
            assert_eq!(!a, b, "only one backend should be spawned!");
        }
        other => {
            tracing::error!(
                ?other,
                concat!(
                    "simultaneous schedule requests failed to produce two schedule",
                    "responses with one spawned and the other retrieved"
                )
            );
        }
    }

    let drone_id_2 = DroneId::new_random();
    let mock_agent_2 = MockAgent::new(nats_conn.clone(), &drone_id_2, state.clone()).await;
    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id_2.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new(CLUSTER_DOMAIN).clone(),
            drone_id: drone_id_2.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;

    let speedlock2: ResourceLock = "speedlock2".to_string().try_into().unwrap();
    //testing simultaneous schedule requests to multiple drones
    match tokio::join!(
        mock_agent.schedule_drone(false, Some(speedlock2.clone())),
        mock_agent_2.schedule_drone(false, Some(speedlock2))
    ) {
        (
            Ok(ScheduleResponse::Scheduled { spawned: a, .. }),
            Ok(ScheduleResponse::Scheduled { spawned: b, .. }),
        ) => {
            assert_eq!(!a, b, "only one backend should be spawned!");
        }
        other => {
            tracing::error!(
                ?other,
                concat!(
                    "simultaneous schedule requests failed to produce two schedule",
                    "responses with one spawned and the other retrieved"
                )
            );
        }
    }
}

#[integration_test]
async fn fetch_locked_backend() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;

    let foobar: ResourceLock = "foobar".to_string().try_into().unwrap();
    let r1 = mock_agent
        .schedule_drone(false, Some(foobar.clone()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled {
        drone: drone1,
        backend_id: backend1,
        ..
    } = r1
    else {
        panic!()
    };

    let r2 = mock_agent
        .fetch_locked(ClusterName::new("plane.test"), foobar)
        .await
        .unwrap();

    let FetchBackendForLockResponse::Scheduled {
        drone: drone2,
        backend_id: backend2,
        ..
    } = r2
    else {
        panic!()
    };

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);
}

#[integration_test]
async fn schedule_request_lock_with_bearer_token() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let drone_ready = drone_ready_notify(
        state.clone(),
        drone_id.clone(),
        ClusterName::new(CLUSTER_DOMAIN),
    );

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new(CLUSTER_DOMAIN).clone(),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.await;

    let foobar: ResourceLock = "foobar".to_string().try_into().unwrap();
    let r1 = mock_agent
        .schedule_drone(true, Some(foobar.clone()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled {
        drone: drone1,
        backend_id: backend1,
        bearer_token: bearer_token1,
        ..
    } = r1
    else {
        panic!()
    };
    assert!(bearer_token1.is_some());

    let r2 = mock_agent.schedule_drone(true, Some(foobar)).await.unwrap();

    let ScheduleResponse::Scheduled {
        drone: drone2,
        backend_id: backend2,
        bearer_token: bearer_token2,
        ..
    } = r2
    else {
        panic!()
    };

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);
    assert_eq!(bearer_token1, bearer_token2);
}
