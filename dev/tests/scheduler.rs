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
        scheduler::{ScheduleResponse, Resource},
    },
    nats::TypedNats,
    state::{start_state_loop, StateHandle},
    types::{BackendId, ClusterName, DroneId},
    NeverResult,
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, timeout, LivenessGuard},
    util::{base_scheduler_request, random_loopback_ip},
};
use std::{net::IpAddr, time::Duration};
use tokio::time::sleep;

pub const CLUSTER_DOMAIN: &str = "plane.test";
const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct MockAgent {
    nats: TypedNats,
    state: StateHandle,
    _state_monitor: LivenessGuard<NeverResult>,
}

impl MockAgent {
    pub async fn new(nats: TypedNats, drone_id: &DroneId) -> Self {
        let ip: IpAddr = random_loopback_ip().into();
        let cluster = ClusterName::new(CLUSTER_DOMAIN);
        let request = DroneConnectRequest {
            drone_id: drone_id.clone(),
            cluster: cluster.clone(),
            ip,
            version: Some("0.1.0".to_string()),
            git_hash: None,
        };

        let state = start_state_loop(nats.clone()).await.unwrap();

        let state_monitor = expect_to_stay_alive(monitor_drone_state(nats.clone()));

        tokio::time::sleep(Duration::from_millis(100)).await;
        let result = nats.request(&request).await.unwrap();
        assert_eq!(true, result, "Drone connect request should succeed.");
        tokio::time::sleep(Duration::from_millis(100)).await;

        MockAgent {
            nats,
            state,
            _state_monitor: state_monitor,
        }
    }

    pub async fn schedule_drone(
        &self,
        drone_id: &DroneId,
        request_bearer_token: bool,
    ) -> Result<ScheduleResponse> {
        // Subscribe to spawn requests for this drone, to ensure that the
        // scheduler sends them.
        let cluster = ClusterName::new(CLUSTER_DOMAIN);
        let mut sub = self
            .nats
            .subscribe(SpawnRequest::subscribe_subject(&cluster, drone_id))
            .await?;
        sleep(Duration::from_millis(100)).await;

        // Construct a scheduler request.
        let mut request = base_scheduler_request();
        let Resource::Backend(ref mut resource) = request.resource else { panic!(); };
		resource.require_bearer_token = request_bearer_token;

        // Publish scheduler request, but defer waiting for response.
        let mut response_handle = self.nats.split_request(&request).await?;

        // Expect a spawn request from the scheduler.
        let result = timeout(1_000, "Agent should receive spawn request.", sub.next())
            .await?
            .unwrap();

        // Ensure that the SpawnRequest is as expected.
        assert_eq!(
            drone_id, &result.value.drone_id,
            "Scheduled drone did not match expectation."
        );

        // Acting as the agent, respond true to indicate successful spawn.
        result.respond(&true).await?;

        // Expect the scheduler to respond.
        let result = timeout(
            1_000,
            "Schedule request should be responded.",
            response_handle.response(),
        )
        .await??;

        // If the schedule request was successful, the state data structure
        // should be updated.
        if let ScheduleResponse::Scheduled { backend_id, .. } = &result {
            sleep(Duration::from_millis(100)).await;

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

        Ok(result)
    }
}

#[integration_test]
async fn no_drone_available() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));
    sleep(Duration::from_millis(100)).await;

    let request = base_scheduler_request();
    tracing::info!("Making spawn request.");
    let result = timeout(
        1_000,
        "Schedule request should be responded.",
        nats_conn.request(&request),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(ScheduleResponse::NoDroneAvailable, result);
}

#[integration_test]
async fn one_drone_available() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();

    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));

    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id).await;

    sleep(Duration::from_millis(100)).await;

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

    sleep(Duration::from_millis(100)).await;
    let result = mock_agent.schedule_drone(&drone_id, false).await.unwrap();
    assert!(matches!(result, ScheduleResponse::Scheduled { drone, .. } if drone == drone_id));
}

#[integration_test]
async fn drone_not_ready() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let drone_id = DroneId::new_random();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));
    sleep(Duration::from_millis(100)).await;

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

    let request = base_scheduler_request();
    tracing::info!("Making spawn request.");
    let result = timeout(
        1_000,
        "Schedule request should be responded.",
        nats_conn.request(&request),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(ScheduleResponse::NoDroneAvailable, result);
}

#[integration_test]
async fn drone_becomes_not_ready() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let drone_id = DroneId::new_random();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));
    sleep(Duration::from_millis(100)).await;

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

    sleep(Duration::from_secs(5)).await;

    let request = base_scheduler_request();
    tracing::info!("Making spawn request.");
    let result = timeout(
        1_000,
        "Schedule request should be responded.",
        nats_conn.request(&request),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ScheduleResponse::NoDroneAvailable, result);
}

#[integration_test]
async fn schedule_request_bearer_token() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id).await;
    sleep(Duration::from_millis(100)).await;

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

    sleep(Duration::from_millis(100)).await;
    let result = mock_agent.schedule_drone(&drone_id, true).await.unwrap();

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
    let _scheduler_guard = expect_to_stay_alive(update_backend_state_loop(nats_conn.clone()));
    sleep(Duration::from_millis(100)).await;

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
