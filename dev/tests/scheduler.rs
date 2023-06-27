use anyhow::Result;
use chrono::Utc;
use integration_test::integration_test;
use lazy_static::lazy_static;
use plane_controller::{
    drone_state::monitor_drone_state, run::update_backend_state_loop, run_scheduler,
};
use plane_core::{
    messages::{
        agent::{BackendState, BackendStateMessage, DroneState, SpawnRequest},
        drone_state::{DroneConnectRequest, DroneStatusMessage, UpdateBackendStateMessage},
        scheduler::ScheduleResponse,
    },
    nats::TypedNats,
    state::{start_state_loop, ClosableNotify, StateHandle},
    types::{BackendId, ClusterName, DroneId},
    NeverResult,
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, timeout, LivenessGuard},
    util::{base_scheduler_request, random_loopback_ip},
};
use std::net::IpAddr;

pub const CLUSTER_DOMAIN: &str = "plane.test";
const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");
lazy_static! {
    static ref PLANE_CLUSTER: ClusterName = ClusterName::new(CLUSTER_DOMAIN);
}

struct MockAgent {
    nats: TypedNats,
    state: StateHandle,
    _state_monitor: LivenessGuard<NeverResult>,
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

        let drone_id_static = Box::leak(Box::new(drone_id.clone()));
        let mut drone_connected = state.state().get_listener_for_predicate(|ws| {
            tracing::info!(?ws, "current ws");
            let cluster = ws.cluster(&PLANE_CLUSTER).unwrap();
            tracing::info!(?cluster, "cluster");
            let drone = cluster.drone(drone_id_static);
            drone.unwrap().meta.is_some()
        });

        let nats2 = nats.clone();
        let (Ok(Ok(result)), _) = tokio::join!(
			tokio::spawn(async move { nats2.request(&request).await }),
			drone_connected.notified()
		) else { panic!() };

        assert_eq!(true, result, "Drone connect request should succeed.");

        MockAgent {
            nats,
            state,
            _state_monitor: state_monitor,
        }
    }

    /// This provides a way to spawn backends without needing different function calls
    /// for spawning vs retrieval.
    pub async fn schedule_drone(
        &self,
        drone_id: &DroneId,
        request_bearer_token: bool,
        lock: Option<String>,
    ) -> Result<ScheduleResponse> {
        let cluster = ClusterName::new(CLUSTER_DOMAIN);
        let mut sub = self
            .nats
            .subscribe(SpawnRequest::subscribe_subject(&cluster, drone_id))
            .await?;
        let mut request = base_scheduler_request();
        request.require_bearer_token = request_bearer_token;
        request.lock = lock.clone();

        let result = self.nats.request(&request);
        let should_spawned = async {
            let Ok(Some(spawn_msg)) = timeout(1_000, "Agent should receive spawn request", sub.next())
                .await else { tracing::info!("does not receive spawn req"); return false };
            assert_eq!(
                drone_id, &spawn_msg.value.drone_id,
                "Scheduled drone did not match expectations"
            );
            spawn_msg.respond(&true).await.unwrap();
            true
        };

        let (res, should_spawned) = tokio::join!(result, should_spawned);
        if let Ok(ScheduleResponse::Scheduled {
            backend_id,
            spawned,
            ..
        }) = &res
        {
            assert_eq!(should_spawned, spawned.clone());
            let state = self.state.state();
            let backend = state
                .cluster(&cluster)
                .expect("Cluster should exist.")
                .backends
                .get(&backend_id)
                .expect("Backend should exist.");
            assert_eq!(
                Some(drone_id),
                backend.drone.as_ref(),
                "Backend should be scheduled on the expected drone."
            );
        }

        tracing::info!("schedule_drone {:?}; lock {:?}", res, lock);

        return res;
    }
}

fn drone_ready_notify(state: StateHandle, drone: DroneId, cluster: ClusterName) -> ClosableNotify {
    let drone = Box::leak(Box::new(drone));
    let cluster = Box::leak(Box::new(cluster));
    state.state().get_listener_for_predicate(|ws| {
        tracing::info!(?ws, "current ws");
        let cluster = ws.cluster(cluster).unwrap();
        tracing::info!(?cluster, "cluster");
        let drone = cluster.drone(drone);
        tracing::info!(?drone, "drone");
        let drone_state = drone.unwrap().state();
        tracing::info!(?drone_state, "drone state");
        drone_state.unwrap() == DroneState::Ready && drone.unwrap().last_seen.is_some()
    })
}
fn drone_not_ready_notify(
    state: StateHandle,
    drone: DroneId,
    cluster: ClusterName,
) -> ClosableNotify {
    let drone = Box::leak(Box::new(drone));
    let cluster = Box::leak(Box::new(cluster));
    state.state().get_listener_for_predicate(|ws| {
        tracing::info!(?ws, "current ws");
        let cluster = ws.cluster(cluster).unwrap();
        tracing::info!(?cluster, "cluster");
        let drone = cluster.drone(drone);
        tracing::info!(?drone, "drone");
        let drone_state = drone.unwrap().state();
        tracing::info!(?drone_state, "drone state");
        drone_state.unwrap() != DroneState::Ready && drone.unwrap().last_seen.is_some()
    })
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

    let mut drone_ready =
        drone_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

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

    drone_ready.notified().await;
    let result = mock_agent
        .schedule_drone(&drone_id, false, None)
        .await
        .unwrap();

    assert!(matches!(&result, ScheduleResponse::Scheduled { drone, .. } if drone == &drone_id));
    let backend_id = match result {
        ScheduleResponse::Scheduled { backend_id, .. } => backend_id,
        _ => panic!("Unexpected schedule response."),
    };

    let backend_id_static = Box::leak(Box::new(backend_id));
    let drone_id_static = Box::leak(Box::new(drone_id.clone()));
    let mut backend_drone_same_as_assigned = state.state().get_listener_for_predicate(|ws| {
        let assigned_drone = ws
            .cluster(&PLANE_CLUSTER)
            .unwrap()
            .backends
            .get(backend_id_static)
            .unwrap()
            .drone
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(drone_id_static.clone(), assigned_drone);
        assigned_drone.id() == drone_id_static.id()
    });

    //tick time
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
    backend_drone_same_as_assigned.notified().await;
}

#[integration_test]
async fn drone_not_ready() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let drone_id = DroneId::new_random();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));

    let mut drone_not_ready =
        drone_not_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

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

    drone_not_ready.notified().await;

    let result = mock_agent
        .schedule_drone(&drone_id, false, None)
        .await
        .unwrap();

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

    let mut drone_ready =
        drone_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

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

    drone_ready.notified().await;

    let mut drone_not_ready =
        drone_not_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

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

    drone_not_ready.notified().await;

    let result = mock_agent
        .schedule_drone(&drone_id, false, None)
        .await
        .unwrap();

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

    let mut drone_ready =
        drone_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

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

    drone_ready.notified().await;

    let result = mock_agent
        .schedule_drone(&drone_id, true, None)
        .await
        .unwrap();

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
    let mut drone_ready =
        drone_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

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

    drone_ready.notified().await;

    let r1 = mock_agent
        .schedule_drone(&drone_id, false, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone1, backend_id: backend1, .. } = r1 else {panic!()};

    let r2 = mock_agent
        .schedule_drone(&drone_id, false, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone2, backend_id: backend2, .. } = r2 else {panic!()};

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);

    match tokio::join!(
        mock_agent.schedule_drone(&drone_id, false, Some("speedlock".to_string())),
        mock_agent.schedule_drone(&drone_id, false, Some("speedlock".to_string()))
    ) {
        (
            Ok(ScheduleResponse::Scheduled { spawned: a, .. }),
            Ok(ScheduleResponse::Scheduled { spawned: b, .. }),
        ) => assert_eq!(!a, b),
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
async fn schedule_request_lock_with_bearer_token() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id, state.clone()).await;
    let mut drone_ready =
        drone_ready_notify(state.clone(), drone_id.clone(), PLANE_CLUSTER.clone());

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: PLANE_CLUSTER.clone(),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

    drone_ready.notified().await;

    let r1 = mock_agent
        .schedule_drone(&drone_id, true, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone1, backend_id: backend1, bearer_token: bearer_token1, .. } = r1 else {panic!()};
    assert!(bearer_token1.is_some());

    let r2 = mock_agent
        .schedule_drone(&drone_id, true, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone2, backend_id: backend2, bearer_token: bearer_token2, .. } = r2 else {panic!()};

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);
    assert_eq!(bearer_token1, bearer_token2);
}
