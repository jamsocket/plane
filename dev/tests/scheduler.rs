use anyhow::Result;
use chrono::Utc;
use integration_test::integration_test;
use plane_controller::{run::update_backend_state_loop, run_scheduler};
use plane_core::{
    messages::{
        agent::{
            BackendState, BackendStateMessage, DroneState, DroneStatusMessage, SpawnRequest,
            UpdateBackendStateMessage,
        },
        scheduler::ScheduleResponse,
    },
    nats::TypedNats,
    types::{BackendId, ClusterName, DroneId},
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, timeout},
    util::base_scheduler_request,
};
use std::time::Duration;
use tokio::time::sleep;

const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct MockAgent {
    nats: TypedNats,
}

impl MockAgent {
    pub fn new(nats: TypedNats) -> Self {
        MockAgent { nats }
    }

    pub async fn schedule_drone(
        &self,
        drone_id: &DroneId,
        request_bearer_token: bool,
    ) -> Result<ScheduleResponse> {
        // Subscribe to spawn requests for this drone, to ensure that the
        // scheduler sends them.
        let mut sub = self
            .nats
            .subscribe(SpawnRequest::subscribe_subject(drone_id))
            .await?;
        sleep(Duration::from_millis(100)).await;

        // Construct a scheduler request.
        let mut request = base_scheduler_request();
        request.require_bearer_token = request_bearer_token;

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

        Ok(result)
    }
}

#[integration_test]
async fn no_drone_available() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
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
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone());
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
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

    let result = mock_agent.schedule_drone(&drone_id, false).await.unwrap();
    assert!(matches!(result, ScheduleResponse::Scheduled { drone, .. } if drone == drone_id));
}

#[integration_test]
async fn drone_not_ready() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let drone_id = DroneId::new_random();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
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
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
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
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone());
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
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
            cluster: Some(ClusterName::new("plane.test")),
        },
        result.0
    );
}
