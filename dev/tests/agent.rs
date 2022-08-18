use anyhow::Result;
use dev::{
    resources::nats::Nats,
    scratch_dir,
    timeout::{expect_to_stay_alive, timeout, LivenessGuard},
    util::{random_backend_id, random_loopback_ip},
};
use dis_spawner::{
    messages::agent::{
        BackendState, BackendStateMessage, DroneConnectRequest, DroneConnectResponse,
        DroneStatusMessage, SpawnRequest,
    },
    nats::{NoReply, TypedNats, TypedSubscription},
    nats_connection::NatsConnection,
    types::{BackendId, DroneId},
};
use dis_spawner_drone::{
    database_connection::DatabaseConnection,
    drone::{
        agent::{AgentOptions, DockerOptions},
        cli::IpProvider,
    },
};
use integration_test::integration_test;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

const TEST_IMAGE: &str = "ghcr.io/drifting-in-space/test-image:latest";
const CLUSTER_DOMAIN: &str = "spawner.test";

struct Agent {
    #[allow(unused)]
    agent_guard: LivenessGuard,
    pub ip: Ipv4Addr,
}

impl Agent {
    pub async fn new(nats: &Nats) -> Result<Agent> {
        let ip = random_loopback_ip();

        let agent_opts = AgentOptions {
            db: DatabaseConnection::new(
                scratch_dir("agent")
                    .join("drone.db")
                    .to_str()
                    .unwrap()
                    .to_string(),
            ),
            nats: NatsConnection::new(nats.connection_string())?,
            cluster_domain: CLUSTER_DOMAIN.into(),
            ip: IpProvider::Literal(IpAddr::V4(ip)),
            docker_options: DockerOptions::default(),
        };

        let agent_guard =
            expect_to_stay_alive(dis_spawner_drone::drone::agent::run_agent(agent_opts));

        Ok(Agent { agent_guard, ip })
    }
}

struct MockController {
    nats: TypedNats,
    drone_connect_response_subscription:
        TypedSubscription<DroneConnectRequest, DroneConnectResponse>,
}

impl MockController {
    pub async fn new(nats: TypedNats) -> Result<Self> {
        let drone_connect_response_subscription =
            nats.subscribe(DroneConnectRequest::subject()).await?;

        Ok(MockController {
            nats,
            drone_connect_response_subscription,
        })
    }

    /// Complete the initial handshake between the drone and the platform, mocking the
    /// platform.
    pub async fn expect_handshake(&mut self, drone_id: DroneId, expect_ip: Ipv4Addr) -> Result<()> {
        let message = timeout(
            30_000,
            "Should receive drone connect message.",
            self.drone_connect_response_subscription.next(),
        )
        .await?
        .unwrap();

        assert_eq!(CLUSTER_DOMAIN, message.value.cluster);
        assert_eq!(expect_ip, message.value.ip);

        message
            .respond(&DroneConnectResponse::Success { drone_id })
            .await?;

        Ok(())
    }

    /// Expect to receive a status message from a live drone.
    pub async fn expect_status_message(&self, drone_id: DroneId, cluster: &str) -> Result<()> {
        let mut status_sub = self
            .nats
            .subscribe(DroneStatusMessage::subject(drone_id))
            .await?;

        let message = timeout(
            6_000,
            "Should receive status message from drone.",
            status_sub.next(),
        )
        .await?
        .unwrap();

        assert_eq!(drone_id, message.value.drone_id);
        assert_eq!(cluster, message.value.cluster);

        Ok(())
    }
}

struct BackendStateSubscription {
    sub: TypedSubscription<BackendStateMessage, NoReply>,
}

impl BackendStateSubscription {
    pub async fn new(nats: &TypedNats, backend_id: &BackendId) -> Result<Self> {
        let sub = nats
            .subscribe(BackendStateMessage::subject(backend_id))
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
            .await
            .unwrap()
            .unwrap()
            .value
            .state
        );

        Ok(())
    }
}

#[integration_test]
async fn drone_sends_status_messages() -> Result<()> {
    let nats = Nats::new().await?;
    let agent = Agent::new(&nats).await?;
    let mut controller_mock = MockController::new(nats.connection().await?).await?;

    let drone_id = DroneId::new(345);
    controller_mock.expect_handshake(drone_id, agent.ip).await?;

    controller_mock
        .expect_status_message(drone_id, "spawner.test")
        .await?;
    controller_mock
        .expect_status_message(drone_id, "spawner.test")
        .await?;
    controller_mock
        .expect_status_message(drone_id, "spawner.test")
        .await?;

    Ok(())
}

#[integration_test]
async fn spawn_with_agent() -> Result<()> {
    let nats = Nats::new().await?;
    let agent = Agent::new(&nats).await?;
    let nats = nats.connection().await?;
    let mut controller_mock = MockController::new(nats.clone()).await?;

    let drone_id = DroneId::new(345);
    controller_mock.expect_handshake(drone_id, agent.ip).await?;

    controller_mock
        .expect_status_message(drone_id, "spawner.test")
        .await?;

    let backend_id = random_backend_id("spawn-with-agent");
    let request = SpawnRequest {
        image: TEST_IMAGE.into(),
        backend_id: backend_id.clone(),
        max_idle_secs: Duration::from_secs(10),
        env: vec![("PORT".into(), "8080".into())].into_iter().collect(),
        metadata: vec![("foo".into(), "bar".into())].into_iter().collect(),
        credentials: None,
    };

    let mut state_subscription = BackendStateSubscription::new(&nats, &backend_id).await?;

    let result = timeout(
        10_000,
        "Spawn request acknowledged by agent.",
        nats.request(&SpawnRequest::subject(drone_id), &request),
    )
    .await?;

    assert!(result, "Spawn request should result in response of _true_.");

    state_subscription
        .expect_backend_status_message(BackendState::Loading, 30_000)
        .await?;
    state_subscription
        .expect_backend_status_message(BackendState::Starting, 5_000)
        .await?;
    state_subscription
        .expect_backend_status_message(BackendState::Ready, 50_000)
        .await?;

    Ok(())
}
