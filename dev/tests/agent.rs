use anyhow::{anyhow, Result};
use integration_test::integration_test;
use plane_core::{
    messages::{
        agent::{
            BackendState, BackendStateMessage, BackendStatsMessage, DroneConnectRequest,
            DroneStatusMessage, SpawnRequest, TerminationRequest,
        },
        dns::{DnsRecordType, SetDnsRecord},
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
    util::{base_spawn_request, random_loopback_ip},
};
use plane_drone::config::DockerConfig;
use plane_drone::{agent::AgentOptions, database::DroneDatabase, ip::IpSource};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use tokio::time::{sleep, Instant};

const CLUSTER_DOMAIN: &str = "plane.test";

struct Agent {
    #[allow(unused)]
    agent_guard: LivenessGuard<NeverResult>,
    pub ip: Ipv4Addr,
    pub db: DroneDatabase,
}

impl Agent {
    pub async fn new(nats: &Nats, drone_id: &DroneId) -> Result<Agent> {
        let ip = random_loopback_ip();
        let db = DroneDatabase::new(&scratch_dir("agent").join("drone.db")).await?;

        let agent_opts = AgentOptions {
            db: db.clone(),
            drone_id: drone_id.clone(),
            nats: nats.connection().await?,
            cluster_domain: ClusterName::new(CLUSTER_DOMAIN),
            ip: IpSource::Literal(IpAddr::V4(ip)),
            docker_options: DockerConfig::default(),
        };

        let agent_guard = expect_to_stay_alive(plane_drone::agent::run_agent(agent_opts));

        Ok(Agent {
            agent_guard,
            ip,
            db,
        })
    }
}

struct MockController {
    nats: TypedNats,
    drone_connect_response_subscription: TypedSubscription<DroneConnectRequest>,
    dns_subscription: TypedSubscription<SetDnsRecord>,
}

impl MockController {
    pub async fn new(nats: TypedNats) -> Result<Self> {
        let drone_connect_response_subscription = nats
            .subscribe(DroneConnectRequest::subscribe_subject())
            .await?;

        let dns_subscription = nats.subscribe(SetDnsRecord::subscribe_subject()).await?;

        sleep(Duration::from_secs(2)).await;

        Ok(MockController {
            nats,
            drone_connect_response_subscription,
            dns_subscription,
        })
    }

    pub async fn next_dns_record(&mut self) -> Result<SetDnsRecord> {
        timeout(
            5_000,
            "Should receive DNS message.",
            self.dns_subscription.next(),
        )
        .await?
        .map(|d| d.value)
        .ok_or_else(|| anyhow!("Expected a DNS record."))
    }

    /// Complete the initial handshake between the drone and the platform, mocking the
    /// platform.
    pub async fn expect_handshake(
        &mut self,
        expect_drone_id: &DroneId,
        expect_ip: Ipv4Addr,
    ) -> Result<()> {
        let message = timeout(
            5_000,
            "Should receive drone connect message.",
            self.drone_connect_response_subscription.next(),
        )
        .await?
        .unwrap();

        assert_eq!(ClusterName::new(CLUSTER_DOMAIN), message.value.cluster);
        assert_eq!(expect_ip, message.value.ip);
        assert_eq!(expect_drone_id, &message.value.drone_id);

        Ok(())
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
    sub: TypedSubscription<BackendStateMessage>,
}

impl BackendStateSubscription {
    pub async fn new(nats: &TypedNats, backend_id: &BackendId) -> Result<Self> {
        let sub = nats
            .subscribe(BackendStateMessage::subscribe_subject(backend_id))
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
async fn drone_sends_status_messages() -> Result<()> {
    let nats = Nats::new().await?;
    let mut controller_mock = MockController::new(nats.connection().await?).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;

    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;
    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;
    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    Ok(())
}

#[integration_test]
async fn drone_sends_draining_status() -> Result<()> {
    let nats = Nats::new().await?;
    let nats_connection = nats.connection().await?;
    let mut controller_mock = MockController::new(nats_connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;

    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    timeout(
        1_000,
        "Did not receive DrainDrone response",
        nats_connection.request(&DrainDrone {
            cluster: ClusterName::new(CLUSTER_DOMAIN),
            drone: drone_id.clone(),
            drain: true,
        }),
    )
    .await??;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), false, 0)
        .await?;

    Ok(())
}

#[integration_test]
async fn spawn_with_agent() -> Result<()> {
    let nats = Nats::new().await?;
    let connection = nats.connection().await?;
    let mut controller_mock = MockController::new(connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;
    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    let mut request = base_spawn_request();
    request.drone_id = drone_id.clone();

    let mut state_subscription =
        BackendStateSubscription::new(&connection, &request.backend_id).await?;
    controller_mock.spawn_backend(&request).await?;

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 5_000)
        .await?;
    state_subscription
        .expect_backend_status_message(BackendState::Starting, 30_000)
        .await?;
    state_subscription
        .expect_backend_status_message(BackendState::Ready, 5_000)
        .await?;

    let dns_record = controller_mock.next_dns_record().await?;
    assert_eq!(
        SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::A,
            name: request.backend_id.to_string(),
            value: agent.ip.to_string(),
        },
        dns_record
    );

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 1)
        .await?;

    let proxy_route = agent
        .db
        .get_proxy_route(request.backend_id.id())
        .await?
        .expect("Expected proxy route.");
    let result = reqwest::get(format!("http://{}/", proxy_route)).await?;
    assert_eq!("Hello World!", result.text().await?);

    state_subscription
        .expect_backend_status_message(BackendState::Swept, 15_000)
        .await?;

    // Route is invalidated after sweeping.
    assert!(
        agent
            .db
            .get_proxy_route(request.backend_id.id())
            .await?
            .is_none(),
        "Expected proxy route to be swept after backend is stopped."
    );

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    Ok(())
}

#[integration_test]
async fn stats_are_acquired() -> Result<()> {
    let nats = Nats::new().await?;
    let connection = nats.connection().await?;
    let mut controller_mock = MockController::new(connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;
    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;
    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    let mut request = base_spawn_request();
    request.drone_id = drone_id;
    // Ensure long enough life to report stats.
    request.max_idle_secs = Duration::from_secs(30);

    let mut state_subscription =
        BackendStateSubscription::new(&connection, &request.backend_id).await?;
    controller_mock.spawn_backend(&request).await?;

    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await?;

    let mut stats_subscription = connection
        .subscribe(BackendStatsMessage::subscribe_subject(&request.backend_id))
        .await?;

    let stat = timeout(
        30_000,
        "Waiting for stats message.",
        stats_subscription.next(),
    )
    .await?
    .unwrap();
    assert!(stat.value.cpu_use_percent >= 0.);
    assert!(stat.value.mem_use_percent >= 0.);

    state_subscription
        .wait_for_state(BackendState::Swept, 60_000)
        .await?;
    Ok(())
}

#[integration_test]
async fn use_ip_lookup_api() -> Result<()> {
    let server = Server::new(|_| async { "123.11.22.33".to_string() }).await?;

    let provider = IpSource::Api { api: server.url() };

    let result = provider.get_ip().await?;
    assert_eq!("123.11.22.33".parse::<IpAddr>()?, result);

    Ok(())
}

#[integration_test]
async fn handle_error_during_start() -> Result<()> {
    let nats = Nats::new().await?;
    let connection = nats.connection().await?;
    let mut controller_mock = MockController::new(connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;
    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    let mut request = base_spawn_request();
    request.drone_id = drone_id;
    // Exit with error code 1 after 100ms.
    request
        .executable
        .env
        .insert("EXIT_CODE".into(), "1".into());
    request
        .executable
        .env
        .insert("EXIT_TIMEOUT".into(), "100".into());

    let mut state_subscription =
        BackendStateSubscription::new(&connection, &request.backend_id).await?;
    controller_mock.spawn_backend(&request).await?;

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 5_000)
        .await?;
    state_subscription
        .expect_backend_status_message(BackendState::Starting, 30_000)
        .await?;
    state_subscription
        .expect_backend_status_message(BackendState::ErrorStarting, 5_000)
        .await?;

    Ok(())
}

#[integration_test]
async fn handle_failure_after_ready() -> Result<()> {
    let nats = Nats::new().await?;
    let connection = nats.connection().await?;
    let mut controller_mock = MockController::new(connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;
    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    let mut request = base_spawn_request();
    request.drone_id = drone_id;

    let mut state_subscription =
        BackendStateSubscription::new(&connection, &request.backend_id).await?;
    controller_mock.spawn_backend(&request).await?;

    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await?;

    let proxy_route = agent
        .db
        .get_proxy_route(request.backend_id.id())
        .await?
        .expect("Expected proxy route.");
    // A get request to this URL will cause the container to exit with status 1.
    // We don't check the status, because the request itself is expected to fail
    // (the process exits immediately, so the response is not sent).
    let _ = reqwest::get(format!("http://{}/exit/1", proxy_route)).await;

    state_subscription
        .expect_backend_status_message(BackendState::Failed, 5_000)
        .await?;

    Ok(())
}

#[integration_test]
async fn handle_successful_termination() -> Result<()> {
    let nats = Nats::new().await?;
    let connection = nats.connection().await?;
    let mut controller_mock = MockController::new(connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;
    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;

    controller_mock
        .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    let mut request = base_spawn_request();
    request.drone_id = drone_id;

    let mut state_subscription =
        BackendStateSubscription::new(&connection, &request.backend_id).await?;
    controller_mock.spawn_backend(&request).await?;

    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await?;

    let proxy_route = agent
        .db
        .get_proxy_route(request.backend_id.id())
        .await?
        .expect("Expected proxy route.");
    // A get request to this URL will cause the container to exit with status 1.
    // We don't check the status, because the request itself is expected to fail
    // (the process exits immediately, so the response is not sent).
    let _ = reqwest::get(format!("http://{}/exit/0", proxy_route)).await;

    state_subscription
        .expect_backend_status_message(BackendState::Exited, 5_000)
        .await?;

    Ok(())
}

#[integration_test]
async fn handle_agent_restart() -> Result<()> {
    let nats_con = Nats::new().await?;
    let nats = nats_con.connection().await?;
    let mut controller_mock = MockController::new(nats.clone()).await?;

    let mut state_subscription = {
        let drone_id = DroneId::new_random();
        let agent = Agent::new(&nats_con, &drone_id).await?;
        controller_mock
            .expect_handshake(&drone_id, agent.ip)
            .await?;

        controller_mock
            .expect_status_message(&drone_id, &ClusterName::new("plane.test"), true, 0)
            .await?;

        let mut request = base_spawn_request();
        request.drone_id = drone_id;

        let mut state_subscription =
            BackendStateSubscription::new(&nats, &request.backend_id).await?;
        controller_mock.spawn_backend(&request).await?;
        state_subscription
            .wait_for_state(BackendState::Ready, 60_000)
            .await?;
        state_subscription
    };

    // Original agent goes away when it goes out of scope.
    {
        let drone_id = DroneId::new_random();
        let agent = Agent::new(&nats_con, &drone_id).await?;
        controller_mock
            .expect_handshake(&drone_id, agent.ip)
            .await?;

        state_subscription
            .wait_for_state(BackendState::Swept, 20_000)
            .await?;
    }

    Ok(())
}

#[integration_test]
async fn handle_termination_request() -> Result<()> {
    let nats = Nats::new().await?;
    let connection = nats.connection().await?;
    let mut controller_mock = MockController::new(connection.clone()).await?;
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await?;

    let mut request = base_spawn_request();
    // Ensure spawnee lives long enough to be terminated.
    request.drone_id = drone_id.clone();
    request.max_idle_secs = Duration::from_secs(10000);

    controller_mock
        .expect_handshake(&drone_id, agent.ip)
        .await?;
    controller_mock
        .expect_status_message(&request.drone_id, &ClusterName::new("plane.test"), true, 0)
        .await?;

    request.max_idle_secs = Duration::from_secs(1000);
    let mut state_subscription =
        BackendStateSubscription::new(&connection, &request.backend_id).await?;

    controller_mock.spawn_backend(&request).await?;
    state_subscription
        .wait_for_state(BackendState::Ready, 60_000)
        .await?;

    let termination_request = TerminationRequest {
        backend_id: request.backend_id.clone(),
        cluster_id: ClusterName::new(CLUSTER_DOMAIN),
    };
    controller_mock
        .terminate_backend(&termination_request)
        .await?;

    state_subscription
        .wait_for_state(BackendState::Failed, 60_000)
        .await?;
    Ok(())
}
