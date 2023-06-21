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
        scheduler::ScheduleResponse,
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
use lazy_static::lazy_static;

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
        lock: Option<String>,
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
        request.require_bearer_token = request_bearer_token;
        request.lock = lock.clone();

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
        if let ScheduleResponse::Scheduled {
            backend_id,
            spawned,
            ..
        } = &result
        {
            sleep(Duration::from_millis(100)).await;
            assert!(spawned);

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
        tracing::info!("schedule_drone {:?}; lock {:?}", result, lock);

        Ok(result)
    }

	/// This provides a way to spawn backends without needing different function calls
	/// for spawning vs retrieval.
    pub async fn schedule_drone_no_split(
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
                .await else { return false };
            assert_eq!(drone_id, &spawn_msg.value.drone_id,
                       "Scheduled drone did not match expectations");
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

    /// This is poorly named and should be refactored. Essentially, it attempts to
    /// spawn a backend with a lock that is already in use, so it does not need
    /// to mock a drone.
    pub async fn schedule_locked(
        &self,
        request_bearer_token: bool,
        lock: Option<String>,
    ) -> Result<ScheduleResponse> {
        // Construct a scheduler request.
        let mut request = base_scheduler_request();
        request.require_bearer_token = request_bearer_token;
        request.lock = lock.clone();

        // Publish scheduler request, but defer waiting for response.
        let result = self.nats.request(&request).await.unwrap();

        println!("schedule_locked {:?}; lock {:?}", result, lock.clone());
        match result {
            ScheduleResponse::Scheduled { spawned, .. } => {
                assert_eq!(false, spawned, "Backend should not be spawned.");
            }
            _ => {
                panic!("Expected ScheduleResponse::Scheduled, got {:?}", result);
            }
        }
        println!("{:?}; lock {:?}", result, lock.clone());

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
    let result = mock_agent
        .schedule_drone(&drone_id, false, None)
        .await
        .unwrap();
    assert!(matches!(&result, ScheduleResponse::Scheduled { drone, .. } if drone == &drone_id));
    let backend_id = match result {
        ScheduleResponse::Scheduled { backend_id, .. } => backend_id,
        _ => panic!("Unexpected schedule response."),
    };

    // Confirm that the assignment is saved in the state.
    let state2 = start_state_loop(nats_conn.clone()).await.unwrap();

    let assigned_drone = state2
        .state()
        .cluster(&ClusterName::new("plane.test"))
        .unwrap()
        .backends
        .get(&backend_id)
        .unwrap()
        .drone
        .as_ref()
        .unwrap()
        .clone();

    assert_eq!(drone_id, assigned_drone);
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

#[integration_test]
async fn schedule_request_lock() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state));
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone(), &drone_id).await;
    //sleep(Duration::from_millis(100)).await;

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

    //sleep(Duration::from_millis(100)).await;
    let r1 = mock_agent
        .schedule_drone(&drone_id, false, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone1, backend_id: backend1, .. } = r1 else {panic!()};

    //tokio::time::sleep(Duration::from_millis(100)).await;

    let r2 = mock_agent
        .schedule_locked(true, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone2, backend_id: backend2, .. } = r2 else {panic!()};

    match tokio::join!(
        mock_agent.schedule_drone_no_split(&drone_id, false, Some("speedlock".to_string())),
        mock_agent.schedule_drone_no_split(&drone_id, false, Some("speedlock".to_string()))
    ) {
        (
            Ok(ScheduleResponse::Scheduled { spawned: a, .. }),
            Ok(ScheduleResponse::Scheduled { spawned: b, .. }),
        ) => assert_eq!(!a, b),
        other => {
			tracing::error!(
				?other,
				concat!("simultaneous schedule requests failed to produce two schedule",
					"responses with one spawned and the other retrieved"));
			panic!()
		}
    }

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);
}

lazy_static! {
	static ref DRONE_ID: DroneId = DroneId::new_random();
	static ref PLANE_CLUSTER: ClusterName = ClusterName::new("plane.test");
}

#[integration_test]
async fn schedule_request_lock_with_bearer_token() {
    let nats = Nats::new().await.unwrap();
    let nats_conn = nats.connection().await.unwrap();
    let state = start_state_loop(nats_conn.clone()).await.unwrap();
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone(), state.clone()));
    let mock_agent = MockAgent::new(nats_conn.clone(), &DRONE_ID).await;
	let mut drone_ready = state.state().get_listener_for_predicate(|ws| {
		tracing::info!(?ws, "current ws");
		let cluster = ws.cluster(&PLANE_CLUSTER).unwrap();
		tracing::info!(?cluster, "cluster");
		let drone = cluster.drone(&DRONE_ID);
		tracing::info!(?drone, "drone");
		let drone_state = drone.unwrap().state();
		tracing::info!(?drone_state, "drone state");
		drone_state.unwrap() == DroneState::Ready
	});


    nats_conn
        .publish(&DroneStatusMessage {
            cluster: PLANE_CLUSTER.clone(),
            drone_id: DRONE_ID.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: None,
        })
        .await
        .unwrap();

	drone_ready.notified().await;

    let r1 = mock_agent
        .schedule_drone_no_split(&DRONE_ID, true, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone1, backend_id: backend1, bearer_token: bearer_token1, .. } = r1 else {panic!()};
    assert!(bearer_token1.is_some());


    let r2 = mock_agent
        .schedule_drone_no_split(&DRONE_ID, true, Some("foobar".to_string()))
        .await
        .unwrap();

    let ScheduleResponse::Scheduled { drone: drone2, backend_id: backend2, bearer_token: bearer_token2, .. } = r2 else {panic!()};

    assert_eq!(drone1, drone2);
    assert_eq!(backend1, backend2);
    assert_eq!(bearer_token1, bearer_token2);
}
