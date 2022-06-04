use self::docker::DockerInterface;
use crate::{
    messages::agent::{
        BackendState, BackendStateMessage, DroneConnectRequest, DroneConnectResponse, SpawnRequest,
    },
    nats::TypedNats,
    retry::do_with_retry,
    types::DroneId, database::DroneDatabase, get_db,
};
use anyhow::{anyhow, Result};
use http::Uri;
use hyper::Client;
use std::{net::IpAddr, time::Duration};

mod docker;

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

pub async fn start_backend(
    spawn_request: &SpawnRequest,
    docker: DockerInterface,
    nats: TypedNats,
    host_ip: IpAddr,
) -> Result<String> {
    let subject = BackendStateMessage::subject(spawn_request.backend_id.clone());

    nats.publish(&subject, &BackendStateMessage::new(BackendState::Loading))
        .await?;

    docker.pull_image(&spawn_request.image).await?;

    let backend_id = spawn_request.backend_id.id().to_string();
    let container_name = docker
        .run_container(&backend_id, &spawn_request.image, &spawn_request.env)
        .await?;

    nats.publish(&subject, &BackendStateMessage::new(BackendState::Starting))
        .await?;

    let port = docker
        .get_port(&container_name.container_id)
        .await
        .ok_or_else(|| {
            anyhow!(
                "Couldn't get port of container {}",
                container_name.container_id
            )
        })?;

    tracing::info!(%port, "Got port from container.");
    wait_port_ready(port, host_ip).await?;

    nats.publish(&subject, &BackendStateMessage::new(BackendState::Ready))
        .await?;

    Ok(format!("{}:{}", host_ip, port))
}

pub async fn listen_for_spawn_requests(
    drone_id: DroneId,
    docker: DockerInterface,
    nats: TypedNats,
    host_ip: IpAddr,
    db: DroneDatabase,
) -> Result<()> {
    let mut sub = nats.subscribe(&SpawnRequest::subject(drone_id)).await?;

    loop {
        let req = sub.next().await;

        match req {
            Ok(Some(req)) => {
                let _ = req.respond(&true).await;
                let nats = nats.clone();
                let docker = docker.clone();
                let db = db.clone();

                // Run the backend.
                tokio::spawn(async move {
                    match start_backend(&req.value, docker, nats, host_ip).await {
                        Err(error) => tracing::error!(?error, "Error starting backend."),
                        Ok(addr) => {
                            let _ = db.insert_proxy_route(req.value.backend_id.id(), &addr).await;
                        }
                    }
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
    let nats = TypedNats::connect(&agent_opts.nats_url).await?;
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
        DroneConnectResponse::NoSuchCluster => todo!(),
    }
}
