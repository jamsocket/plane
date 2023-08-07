use anyhow::Result;
use integration_test::integration_test;
use plane_controller::{drone_state::monitor_drone_state, run::update_backend_state_loop};
use plane_core::{
    messages::{
        agent::{
            BackendState, BackendStatsMessage, DroneLogMessage, DroneLogMessageKind, SpawnRequest,
            TerminationRequest,
        },
        drone_state::{DroneStatusMessage, UpdateBackendStateMessage},
        scheduler::DrainDrone,
    },
    nats::{TypedNats, TypedSubscription},
    types::{BackendId, ClusterName, DroneId},
    NeverResult,
};
use plane_dev::{
    resources::nats::Nats,
    resources::server::Server,
    scratch_dir,
    timeout::{expect_to_stay_alive, timeout, LivenessGuard},
    util::{
        base_spawn_request, inactive_image_spawn_request, invalid_image_spawn_request,
        random_loopback_ip,
    },
};
use plane_drone::{agent::AgentOptions, database::DroneDatabase, ip::IpSource};
use plane_drone::{agent::AgentSupervisors, config::DockerConfig};
use serde_json::json;
use std::net::IpAddr;
use std::time::Duration;
use tokio::time::Instant;
use tokio_stream::StreamExt;

pub const CLUSTER_DOMAIN: &str = "plane.test";

struct Agent {
    #[allow(unused)]
    agent_guard: AgentSupervisors,
    pub db: DroneDatabase,
    #[allow(unused)]
    loop_guard: LivenessGuard<NeverResult>,
}

impl Agent {
    pub async fn new(
        nats: &Nats,
        drone_id: &DroneId,
        docker_options: DockerConfig,
    ) -> Result<Agent> {
        let ip = random_loopback_ip();
        let db = DroneDatabase::new(&scratch_dir("agent").join("drone.db")).await?;
        let nc = nats.connection().await?;

        let agent_opts = AgentOptions {
            db: db.clone(),
            drone_id: drone_id.clone(),
            nats: nc.clone(),
            cluster_domain: ClusterName::new(CLUSTER_DOMAIN),
            ip: IpSource::Literal(IpAddr::V4(ip)),
            docker_options,
        };

        let agent_guard = plane_drone::agent::run_agent(agent_opts)
            .await
            .expect("Failed to start agent.");
        let loop_guard = expect_to_stay_alive(update_backend_state_loop(nc));

        Ok(Agent {
            agent_guard,
            db,
            loop_guard,
        })
    }
}

struct MockController {
    nats: TypedNats,
    _state_handle: LivenessGuard<NeverResult>,
}

impl MockController {
    pub async fn new(nats: TypedNats) -> Result<Self> {
        let state_handle = expect_to_stay_alive(monitor_drone_state(nats.clone()));

        Ok(MockController {
            nats,
            _state_handle: state_handle,
        })
    }

    /// Expect to receive a status message from a live drone.
    pub async fn expect_status_message(
        &self,
        drone_id: &DroneId,
        cluster: &ClusterName,
        expect_ready: bool,
        expect_running: u32,
    ) -> Result<()> {
        let mut status_sub = self
            .nats
            .subscribe(DroneStatusMessage::subscribe_subject())
            .await?;

        let message = timeout(
            6_000,
            "Should receive status message from drone.",
            status_sub.next(),
        )
        .await?
        .unwrap();

        assert_eq!(drone_id, &message.value.drone_id);
        assert_eq!(cluster, &message.value.cluster);
        assert_eq!(expect_ready, message.value.ready);
        assert_eq!(expect_running, message.value.running_backends.unwrap());

        Ok(())
    }

    pub async fn spawn_backend(&self, request: &SpawnRequest) -> Result<()> {
        let result = timeout(
            10_000,
            "Spawn request acknowledged by agent.",
            self.nats.request(request),
        )
        .await??;

        assert!(result, "Spawn request should result in response of _true_.");
        Ok(())
    }

    pub async fn terminate_backend(&self, request: &TerminationRequest) -> Result<()> {
        timeout(10_000, "Termination!", self.nats.request(request)).await??;

        Ok(())
    }
}

struct BackendStateSubscription {
    sub: TypedSubscription<UpdateBackendStateMessage>,
}

impl BackendStateSubscription {
    pub async fn new(nats: &TypedNats, backend_id: &BackendId) -> Result<Self> {
        let sub = nats
            .subscribe(UpdateBackendStateMessage::backend_subject(
                &ClusterName::new(CLUSTER_DOMAIN),
                backend_id,
            ))
            .await?;
        Ok(BackendStateSubscription { sub })
    }

    /// Expect a backend status message.
    pub async fn expect_backend_status_message(
        &mut self,
        expected_state: BackendState,
        timeout_ms: u64,
    ) -> Result<()> {
        assert_eq!(
            expected_state,
            timeout(
                timeout_ms,
                &format!("State should become {:?}", expected_state),
                self.sub.next()
            )
            .await?
            .unwrap()
            .value
            .state
        );

        Ok(())
    }

    pub async fn wait_for_state(
        &mut self,
        desired_state: BackendState,
        timeout_ms: u64,
    ) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .unwrap();
        let mut last_state: Option<BackendState> = None;

        loop {
            let next = tokio::time::timeout_at(deadline, self.sub.next()).await;

            match next {
                Ok(Some(v)) => {
                    if v.value.state == desired_state {
                        tracing::info!(?desired_state, "State became desired state.");
                        return Ok(())
                    } else {
                        last_state = Some(v.value.state);
                    }
                },
                Ok(_) => panic!("Stream terminated while waiting for backend state to become {:?} (last state was {:?})", desired_state, last_state),
                Err(_) => panic!("Timed out waiting for backend state to become {:?} (last state was {:?})", desired_state, last_state),
            };
        }
    }
}

#[integration_test]
async fn drone_sends_status_messages() {
    let nats = Nats::new().await.unwrap();
    let controller_mock = MockController::new(nats.connection().await.unwrap())
        .await
        .unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();
    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();
    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();
}

#[integration_test]
async fn drone_sends_draining_status() {
    let nats = Nats::new().await.unwrap();
    let nats_connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(nats_connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    timeout(
        1_000,
        "Did not receive DrainDrone response",
        nats_connection.request(&DrainDrone {
            cluster: ClusterName::new(CLUSTER_DOMAIN),
            drone: drone_id.clone(),
            drain: true,
        }),
    )
    .await
    .unwrap()
    .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), false, 0)
        .await
        .unwrap();
}

#[allow(dead_code)]
struct TestFixture {
    nats_server: Nats,
    nats_connection: TypedNats,
    controller: MockController,
    agent: Agent,
}

async fn do_spawn_request(request: &mut SpawnRequest) -> (TestFixture, BackendStateSubscription) {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();

    request.drone_id = drone_id.clone();

    controller_mock.spawn_backend(request).await.unwrap();
    (
        TestFixture {
            nats_server: nats,
            nats_connection: connection,
            controller: controller_mock,
            agent,
        },
        state_subscription,
    )
}

#[integration_test]
async fn invalid_container_fails() {
    let mut req = invalid_image_spawn_request().await;
    let (ctx, mut sub) = do_spawn_request(&mut req).await;
    let log_subscription = ctx
        .nats_connection
        .subscribe(DroneLogMessage::subscribe_subject(&req.backend_id))
        .await
        .unwrap()
        .timeout(Duration::from_secs(10));
    sub.expect_backend_status_message(BackendState::Loading, 30_000)
        .await
        .unwrap();
    sub.expect_backend_status_message(BackendState::ErrorLoading, 30_000)
        .await
        .unwrap();
    assert_eq!(
        Box::pin(log_subscription)
            .try_next()
            .await
            .unwrap()
            .unwrap()
            .value
            .kind,
        DroneLogMessageKind::Meta
    );
}

#[integration_test]
async fn inactive_container_times_out() {
    let mut req = inactive_image_spawn_request().await;
    let (_ctx, mut sub) = do_spawn_request(&mut req).await;
    sub.expect_backend_status_message(BackendState::Loading, 30_000)
        .await
        .unwrap();
    sub.expect_backend_status_message(BackendState::Starting, 30_000)
        .await
        .unwrap();
    sub.expect_backend_status_message(BackendState::TimedOutBeforeReady, 60_000)
        .await
        .unwrap();
}

#[integration_test]
async fn check_that_logs_work() {
    let mut req = base_spawn_request().await;
    req.executable.env.insert("EXIT_CODE".into(), "2".into());
    req.executable.env.insert("EXIT_TIMEOUT".into(), "5".into());

    let (ctx, mut status_sub) = do_spawn_request(&mut req).await;
    let mut log_subscription = Box::pin(
        ctx.nats_connection
            .subscribe(DroneLogMessage::subscribe_subject(&req.backend_id))
            .await
            .unwrap()
            .timeout(Duration::from_secs(10)),
    );

    status_sub
        .wait_for_state(BackendState::Ready, 2000)
        .await
        .unwrap();

    let proxy_route = ctx
        .agent
        .db
        .get_proxy_route(req.backend_id.id())
        .await
        .unwrap()
        .expect("Expected proxy route.");

    let _result = reqwest::get(format!("http://{}/cgi-bin/logs.cgi", proxy_route.address)).await;

    for _ in 0..3 {
        assert_eq!(
            log_subscription.next().await.unwrap().unwrap().value.kind,
            DroneLogMessageKind::Stderr
        );
    }
    assert_eq!(
        log_subscription.next().await.unwrap().unwrap().value.text,
        "Backend exit code: 2"
    );
}

#[integration_test]
async fn spawn_with_agent() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id.clone();
    const TEST_VAR_NAME: &str = "TEST_VAR"; //note this is hardcoded in buildable-test-server
    const TEST_VAR_VALUE: &str = "TESTING";
    request
        .executable
        .env
        .insert(TEST_VAR_NAME.into(), TEST_VAR_VALUE.into());

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 5_000)
        .await
        .unwrap();
    state_subscription
        .expect_backend_status_message(BackendState::Starting, 30_000)
        .await
        .unwrap();
    state_subscription
        .expect_backend_status_message(BackendState::Ready, 5_000)
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 1)
        .await
        .unwrap();

    let proxy_route = agent
        .db
        .get_proxy_route(request.backend_id.id())
        .await
        .unwrap()
        .expect("Expected proxy route.");

    let result = reqwest::get(format!("http://{}/", proxy_route.address))
        .await
        .unwrap();
    assert_eq!("Hello World!", result.text().await.unwrap().trim());

    let env_var = reqwest::get(format!("http://{}/cgi-bin/env.cgi", proxy_route.address))
        .await
        .unwrap();
    assert_eq!(TEST_VAR_VALUE, env_var.text().await.unwrap().trim());

    state_subscription
        .expect_backend_status_message(BackendState::Swept, 15_000)
        .await
        .unwrap();

    // Route is invalidated after sweeping.
    assert!(
        agent
            .db
            .get_proxy_route(request.backend_id.id())
            .await
            .unwrap()
            .is_none(),
        "Expected proxy route to be swept after backend is stopped."
    );

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();
}

#[integration_test]
async fn stats_are_acquired() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id;
    // Ensure long enough life to report stats.
    request.max_idle_secs = Duration::from_secs(30);

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await
        .unwrap();

    let mut stats_subscription = connection
        .subscribe(BackendStatsMessage::subscribe_subject(&request.backend_id))
        .await
        .unwrap();

    // just make sure this generates an Ok() result
    timeout(
        30_000,
        "Waiting for stats message.",
        stats_subscription.next(),
    )
    .await
    .unwrap()
    .unwrap();

    state_subscription
        .wait_for_state(BackendState::Swept, 60_000)
        .await
        .unwrap();
}

#[integration_test]
async fn use_ip_lookup_api() {
    let server = Server::new(|_| async { "123.11.22.33".to_string() })
        .await
        .unwrap();

    let provider = IpSource::Api { api: server.url() };

    let result = provider.get_ip().await.unwrap();
    assert_eq!("123.11.22.33".parse::<IpAddr>().unwrap(), result);
}

#[integration_test]
async fn handle_error_during_start() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id;
    // Exit with error code 1 after 100ms.
    request
        .executable
        .env
        .insert("EXIT_CODE".into(), "1".into());
    request
        .executable
        .env
        .insert("EXIT_TIMEOUT".into(), "0".into());

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 5_000)
        .await
        .unwrap();
    state_subscription
        .expect_backend_status_message(BackendState::Starting, 30_000)
        .await
        .unwrap();
    state_subscription
        .expect_backend_status_message(BackendState::ErrorStarting, 5_000)
        .await
        .unwrap();
}

#[integration_test]
async fn handle_failure_after_ready() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id;
    request
        .executable
        .env
        .insert("EXIT_CODE".into(), "1".into());
    request
        .executable
        .env
        .insert("EXIT_TIMEOUT".into(), "2".into());
    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await
        .unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Failed, 5_000)
        .await
        .unwrap();
}

#[integration_test]
async fn handle_successful_termination() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id;
    request
        .executable
        .env
        .insert("EXIT_CODE".into(), "0".into());
    request
        .executable
        .env
        .insert("EXIT_TIMEOUT".into(), "2".into());

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await
        .unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Exited, 5_000)
        .await
        .unwrap();
}

#[integration_test]
async fn handle_agent_restart() {
    let nats_con = Nats::new().await.unwrap();
    let nats = nats_con.connection().await.unwrap();
    let controller_mock = MockController::new(nats.clone()).await.unwrap();

    let mut state_subscription = {
        let drone_id = DroneId::new_random();
        let _agent = Agent::new(&nats_con, &drone_id, DockerConfig::default())
            .await
            .unwrap();

        controller_mock
            .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
            .await
            .unwrap();

        let mut request = base_spawn_request().await;
        request.drone_id = drone_id;

        let mut state_subscription = BackendStateSubscription::new(&nats, &request.backend_id)
            .await
            .unwrap();
        controller_mock.spawn_backend(&request).await.unwrap();
        state_subscription
            .wait_for_state(BackendState::Ready, 60_000)
            .await
            .unwrap();
        state_subscription
    };

    // Original agent goes away when it goes out of scope.
    {
        let drone_id = DroneId::new_random();
        let _agent = Agent::new(&nats_con, &drone_id, DockerConfig::default())
            .await
            .unwrap();

        state_subscription
            .wait_for_state(BackendState::Swept, 20_000)
            .await
            .unwrap();
    }
}

#[integration_test]
async fn handle_termination_request() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id.clone();

    controller_mock
        .expect_status_message(&request.drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();

    controller_mock.spawn_backend(&request).await.unwrap();

    let termination_request = TerminationRequest {
        backend_id: request.backend_id.clone(),
        cluster_id: ClusterName::new(CLUSTER_DOMAIN),
    };
    controller_mock
        .terminate_backend(&termination_request)
        .await
        .unwrap();

    state_subscription
        .wait_for_state(BackendState::Terminated, 20_000)
        .await
        .unwrap();
}

#[integration_test]
async fn attempt_to_spawn_with_disallowed_volume_mount() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id, DockerConfig::default())
        .await
        .unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id.clone();
    request.executable.volume_mounts = vec![json!({
        "source": "/foo",
        "target": "/bar",
    })];

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 5_000)
        .await
        .unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::ErrorLoading, 5_000)
        .await
        .unwrap();
}

#[integration_test]
async fn attempt_to_spawn_with_allowed_volume_mount() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(connection.clone()).await.unwrap();
    let drone_id = DroneId::new_random();
    let docker_config = DockerConfig {
        allow_volume_mounts: true,
        ..DockerConfig::default()
    };
    let _agent = Agent::new(&nats, &drone_id, docker_config).await.unwrap();

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await
        .unwrap();

    let mut request = base_spawn_request().await;
    request.drone_id = drone_id.clone();
    request.executable.volume_mounts = vec![json!({
        "Type": "tmpfs",
        "Target": "/bar",
    })];

    let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
        .await
        .unwrap();
    controller_mock.spawn_backend(&request).await.unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 5_000)
        .await
        .unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Starting, 5_000)
        .await
        .unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Ready, 5_000)
        .await
        .unwrap();

    state_subscription
        .expect_backend_status_message(BackendState::Swept, 50_000)
        .await
        .unwrap();
}
