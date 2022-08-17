use anyhow::Result;
use dev::{resources::nats::Nats, scratch_dir, timeout::timeout, util::random_loopback};
use dis_spawner::{
    messages::agent::{DroneConnectRequest, DroneConnectResponse, DroneStatusMessage},
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
use tokio::task::JoinHandle;
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr},
};

struct ShouldStayAlive {
    #[allow(unused)]
    handle: JoinHandle<Result<()>>,
}

impl ShouldStayAlive {
    pub fn new<F>(future: F) -> Self
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let handle = tokio::spawn(async {
            tracing::info!("h1");
            future.await?;

            panic!(
                "Expected future to stay alive until we killed it, but it finished on its own."
            );
        });

        ShouldStayAlive {
            handle
        }
    }
}

#[integration_test]
async fn drone_sends_ready_message() -> Result<()> {
    let nats = Nats::new().await?;
    let conn = nats.connection().await?;
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

    let mut sub = conn.subscribe(DroneConnectRequest::subject()).await?;

    let _agent = ShouldStayAlive::new(dis_spawner_drone::drone::agent::run_agent(agent_opts));

    let message = timeout(30_000, "Should receive drone connect message.", sub.next())
        .await?
        .unwrap();

    assert_eq!("spawner.test", message.value.cluster);
    assert_eq!(IpAddr::from(ip), message.value.ip);

    message
        .respond(&DroneConnectResponse::Success {
            drone_id: DroneId::new(345),
        })
        .await?;
    
    let mut status_sub = conn.subscribe(DroneStatusMessage::subscribe_subject()).await?;

    for _ in 0..2 {
        let message = timeout(6_000, "Should receive status message from drone.", status_sub.next()).await?.unwrap();

        assert_eq!(DroneId::new(345), message.value.drone_id);
        assert_eq!("spawner.test", message.value.cluster);
    }

    Ok(())
}
