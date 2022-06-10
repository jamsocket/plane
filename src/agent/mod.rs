use self::{docker::DockerInterface, executor::Executor};
use crate::{
    database::DroneDatabase,
    get_db,
    messages::agent::{BackendState, DroneConnectRequest, DroneConnectResponse, SpawnRequest},
    nats::TypedNats,
    retry::do_with_retry,
    types::DroneId,
};
use anyhow::{anyhow, Result};
use http::Uri;
use hyper::Client;
use std::{net::IpAddr, sync::Arc, time::Duration};

mod docker;
mod executor;

#[derive(PartialEq, Debug)]
pub enum DockerApiTransport {
    Socket(String),
    Http(String),
}

impl Default for DockerApiTransport {
    fn default() -> Self {
        DockerApiTransport::Socket("/var/run/docker.sock".to_string())
    }
}

#[derive(PartialEq, Debug)]
pub struct DockerOptions {
    pub transport: DockerApiTransport,
    pub runtime: Option<String>,
}

#[derive(PartialEq, Debug)]
pub struct AgentOptions {
    pub db_path: String,
    pub nats_url: String,
    pub cluster_domain: String,

    /// Public IP of the machine the drone is running on.
    pub ip: IpAddr,

    /// Internal IP of the machine the drone is running on.
    pub host_ip: IpAddr,

    pub docker_options: DockerOptions,
}

pub async fn wait_port_ready(port: u16, host_ip: IpAddr) -> Result<()> {
    tracing::info!(port, %host_ip, "Waiting for ready port.");

    let client = Client::new();
    let uri = Uri::from_maybe_shared(format!("http://{}:{}/", host_ip, port))?;

    do_with_retry(|| client.get(uri.clone()), 3000, Duration::from_millis(10)).await?;

    Ok(())
}

pub async fn listen_for_spawn_requests(
    drone_id: DroneId,
    docker: DockerInterface,
    nats: TypedNats,
    host_ip: IpAddr,
    db: DroneDatabase,
) -> Result<()> {
    let mut sub = nats.subscribe(&SpawnRequest::subject(drone_id)).await?;
    let executor = Arc::new(Executor::new(docker, db, nats, host_ip));

    loop {
        let req = sub.next().await;

        match req {
            Ok(Some(req)) => {
                let executor = executor.clone();

                req.respond(&true).await?;
                tokio::spawn(async move {
                    let _ = executor
                        .run_backend(&req.value, BackendState::Loading)
                        .await;
                });
            }
            Ok(None) => return Err(anyhow!("Spawn request subscription closed.")),
            Err(error) => {
                tracing::error!(?error, "Non-fatal error when listening for spawn requests.")
            }
        }
    }
}

pub async fn run_agent(agent_opts: AgentOptions) -> Result<()> {
    let nats = do_with_retry(
        || TypedNats::connect(&agent_opts.nats_url),
        30,
        Duration::from_secs(10),
    )
    .await?;
    let docker = DockerInterface::try_new(&agent_opts.docker_options).await?;
    let db = get_db(&agent_opts.db_path).await?;

    let result = nats
        .request(
            &DroneConnectRequest::subject(),
            &DroneConnectRequest {
                cluster: agent_opts.cluster_domain.to_string(),
                ip: agent_opts.ip,
            },
        )
        .await?;

    match result {
        DroneConnectResponse::Success { drone_id } => {
            listen_for_spawn_requests(drone_id, docker, nats, agent_opts.host_ip, db).await
        }
        DroneConnectResponse::NoSuchCluster => Err(anyhow!(
            "The platform server did not recognize the cluster {}",
            agent_opts.cluster_domain
        )),
    }
}
