use anyhow::Result;
use dev::{
    resources::nats::Nats,
    scratch_dir,
    timeout::{expect_to_stay_alive, timeout, LivenessGuard},
    util::random_loopback,
};
use dis_spawner::{
    messages::agent::{DroneConnectRequest, DroneConnectResponse, DroneStatusMessage},
    nats::TypedNats,
    nats_connection::NatsConnection,
    types::DroneId,
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

struct Agent {
    #[allow(unused)]
    agent_guard: LivenessGuard,
    pub nats: TypedNats,
    pub ip: Ipv4Addr,
}

impl Agent {
    pub async fn new(nats: &Nats) -> Result<Agent> {
        let ip = random_loopback();

        let agent_opts = AgentOptions {
            db: DatabaseConnection::new(
                scratch_dir("agent")
                    .join("drone.db")
                    .to_str()
                    .unwrap()
                    .to_string(),
            ),
            nats: NatsConnection::new(nats.connection_string())?,
            cluster_domain: "spawner.test".into(),
            ip: IpProvider::Literal(IpAddr::V4(ip)),
            host_ip: IpAddr::V4(Ipv4Addr::from([127, 0, 0, 1])),
            docker_options: DockerOptions::default(),
        };

        let agent_guard =
            expect_to_stay_alive(dis_spawner_drone::drone::agent::run_agent(agent_opts));

        Ok(Agent {
            agent_guard,
            nats: nats.connection().await?,
            ip,
        })
    }

    pub async fn expect_handshake(&self) -> Result<()> {
        let mut sub = self.nats.subscribe(DroneConnectRequest::subject()).await?;

        let message = timeout(30_000, "Should receive drone connect message.", sub.next())
            .await?
            .unwrap();

        assert_eq!("spawner.test", message.value.cluster);
        assert_eq!(IpAddr::from(self.ip), message.value.ip);

        message
            .respond(&DroneConnectResponse::Success {
                drone_id: DroneId::new(345),
            })
            .await?;

        Ok(())
    }
}

#[integration_test]
async fn drone_sends_status_messages() -> Result<()> {
    let nats = Nats::new().await?;
    let agent = Agent::new(&nats).await?;

    agent.expect_handshake().await?;

    let mut status_sub = agent
        .nats
        .subscribe(DroneStatusMessage::subscribe_subject())
        .await?;

    for _ in 0..2 {
        let message = timeout(
            6_000,
            "Should receive status message from drone.",
            status_sub.next(),
        )
        .await?
        .unwrap();

        assert_eq!(DroneId::new(345), message.value.drone_id);
        assert_eq!("spawner.test", message.value.cluster);
    }

    Ok(())
}
