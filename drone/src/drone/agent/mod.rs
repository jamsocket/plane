use self::{docker::DockerInterface, executor::Executor};
use crate::{database_connection::DatabaseConnection, drone::cli::IpProvider};
use anyhow::{anyhow, Result};
use dis_spawner::{
    logging::LogError,
    messages::{
        agent::{
            DroneConnectRequest, DroneConnectResponse, DroneStatusMessage, SpawnRequest,
            TerminationRequest,
        },
        scheduler::ClusterId,
    },
    nats::TypedNats,
    nats_connection::NatsConnection,
    retry::do_with_retry,
    types::DroneId,
};
use http::Uri;
use hyper::Client;
use std::{net::IpAddr, time::Duration};

mod docker;
mod executor;

#[derive(PartialEq, Eq, Debug)]
pub enum DockerApiTransport {
    Socket(String),
    Http(String),
}

impl Default for DockerApiTransport {
    fn default() -> Self {
        DockerApiTransport::Socket("/var/run/docker.sock".to_string())
    }
}

#[derive(PartialEq, Eq, Debug, Default)]
pub struct DockerOptions {
    pub transport: DockerApiTransport,
    pub runtime: Option<String>,
}

#[derive(PartialEq, Debug)]
pub struct AgentOptions {
    pub db: DatabaseConnection,
    pub nats: NatsConnection,
    pub cluster_domain: ClusterId,

    /// Public IP of the machine the drone is running on.
    pub ip: IpProvider,

    pub docker_options: DockerOptions,
}

pub async fn wait_port_ready(port: u16, host_ip: IpAddr) -> Result<()> {
    tracing::info!(port, %host_ip, "Waiting for ready port.");

    let client = Client::new();
    let uri = Uri::from_maybe_shared(format!("http://{}:{}/", host_ip, port))?;

    do_with_retry(|| client.get(uri.clone()), 3000, Duration::from_millis(10)).await?;

    Ok(())
}

async fn listen_for_spawn_requests(
    drone_id: DroneId,
    executor: Executor,
    nats: TypedNats,
) -> Result<()> {
    let mut sub = nats
        .subscribe(&SpawnRequest::subscribe_subject(drone_id))
        .await?;
    executor.resume_backends().await?;
    tracing::info!("Listening for spawn requests.");

    loop {
        let req = sub.next().await;

        match req {
            Ok(Some(req)) => {
                let executor = executor.clone();

                req.respond(&true).await?;
                tokio::spawn(async move {
                    executor.start_backend(&req.value).await;
                });
            }
            Ok(None) => return Err(anyhow!("Spawn request subscription closed.")),
            Err(error) => {
                tracing::error!(?error, "Non-fatal error when listening for spawn requests.")
            }
        }
    }
}

async fn listen_for_termination_requests(executor: Executor, nats: TypedNats) -> Result<()> {
    let mut sub = nats
        .subscribe(TerminationRequest::subscribe_subject())
        .await?;
    tracing::info!("Listening for termination requests.");
    loop {
        let req = sub.next().await;
        match req {
            Ok(Some(req)) => {
                let executor = executor.clone();

                req.respond(&true).await?;
                tokio::spawn(async move { executor.kill_backend(&req.value).await });
            }
            Ok(None) => return Err(anyhow!("Termination request subscription closed.")),
            Err(error) => {
                tracing::error!(
                    ?error,
                    "Non-fatal error when listening for termination requests."
                )
            }
        }
    }
}

/// Repeatedly publish a status message advertising this drone as available.
async fn ready_loop(nc: TypedNats, drone_id: DroneId, cluster: ClusterId) -> Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(4));

    loop {
        nc.publish(&DroneStatusMessage {
            drone_id,
            capacity: 100,
            cluster: cluster.clone(),
        })
        .await
        .log_error("Error in ready loop.");

        interval.tick().await;
    }
}

pub async fn run_agent(agent_opts: AgentOptions) -> Result<()> {
    let nats = agent_opts.nats.connection().await?;

    tracing::info!("Connecting to Docker.");
    let docker = DockerInterface::try_new(&agent_opts.docker_options).await?;
    tracing::info!("Connecting to sqlite.");
    let db = agent_opts.db.connection().await?;
    let cluster = agent_opts.cluster_domain.clone();
    let ip = agent_opts.ip.get_ip().await?;

    tracing::info!("Requesting drone id.");
    let result = {
        let request = DroneConnectRequest {
            cluster: cluster.clone(),
            ip,
        };
        do_with_retry(|| nats.request(&request), 30, Duration::from_secs(10)).await?
    };

    match result {
        DroneConnectResponse::Success { drone_id } => {
            let executor = Executor::new(docker, db, nats.clone());
            let result = tokio::try_join!(
                ready_loop(nats.clone(), drone_id, cluster.clone()),
                listen_for_spawn_requests(drone_id, executor.clone(), nats.clone()),
                listen_for_termination_requests(executor.clone(), nats.clone())
            );
            result.map(|_| ())
        }
        DroneConnectResponse::NoSuchCluster => Err(anyhow!(
            "The platform server did not recognize the cluster {}",
            agent_opts.cluster_domain
        )),
    }
}
