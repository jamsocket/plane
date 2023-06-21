use self::executor::Executor;
use crate::{
    agent::engines::docker::DockerInterface, config::DockerConfig, database::DroneDatabase,
    ip::IpSource,
};
use anyhow::{anyhow, Result};
use http::Uri;
use hyper::Client;
use plane_core::{
    logging::LogError,
    messages::{
        agent::{DroneState, SpawnRequest, TerminationRequest},
        drone_state::{DroneConnectRequest, DroneStatusMessage},
        scheduler::DrainDrone,
    },
    nats::TypedNats,
    retry::do_with_retry,
    supervisor::Supervisor,
    types::{ClusterName, DroneId},
    NeverResult,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::watch::{self, Receiver, Sender};

const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: Option<&str> = option_env!("GIT_HASH");

mod backend;
mod engine;
mod engines;
mod executor;

#[derive(Clone)]
pub struct AgentOptions {
    pub drone_id: DroneId,
    pub db: DroneDatabase,
    pub nats: TypedNats,
    pub cluster_domain: ClusterName,

    /// Public IP of the machine the drone is running on.
    pub ip: IpSource,

    pub docker_options: DockerConfig,
}

pub async fn wait_port_ready(addr: &SocketAddr) -> Result<()> {
    tracing::info!(%addr, "Waiting for ready port.");

    let client = Client::new();
    let uri = Uri::from_maybe_shared(format!("http://{}:{}/", addr.ip(), addr.port()))?;

    do_with_retry(|| client.get(uri.clone()), 3000, Duration::from_millis(10)).await?;

    Ok(())
}

async fn listen_for_spawn_requests(
    cluster: ClusterName,
    drone_id: DroneId,
    executor: Executor<DockerInterface>,
    nats: TypedNats,
) -> NeverResult {
    let mut sub = nats
        .subscribe(SpawnRequest::subscribe_subject(&cluster, &drone_id))
        .await?;
    executor.resume_backends().await?;
    tracing::info!("Listening for spawn requests.");

    loop {
        let req = sub.next().await;

        match req {
            Some(req) => {
                let executor = executor.clone();
                let (send, recv) = tokio::sync::mpsc::channel(1);
                executor.initialize_listener(req.value.backend_id.clone(), send);

                req.respond(&true).await?;
                tokio::spawn(async move {
                    tracing::info!("spawning {:?}", &req.value.backend_id);
                    executor.start_backend(&req.value, recv).await;
                });
            }
            None => return Err(anyhow!("Spawn request subscription closed.")),
        }
    }
}

async fn listen_for_termination_requests(
    executor: Executor<DockerInterface>,
    nats: TypedNats,
    cluster: ClusterName,
) -> NeverResult {
    let mut sub = nats
        .subscribe(TerminationRequest::subscribe_subject(&cluster))
        .await?;
    tracing::info!("Listening for termination requests.");
    loop {
        let req = sub.next().await;
        match req {
            Some(req) => {
                let executor = executor.clone();

                req.respond(&()).await?;
                tokio::spawn(async move {
                    tracing::info!("terminating {:?}", &req.value.backend_id);
                    let result = executor.kill_backend(&req.value).await;
                    if let Err(err) = result {
                        tracing::warn!(?err, "Executor failed to kill backend.");
                    }
                });
            }
            None => return Err(anyhow!("Termination request subscription closed.")),
        }
    }
}

/// Repeatedly publish a status message advertising this drone as available.
async fn ready_loop(
    nc: TypedNats,
    drone_id: DroneId,
    cluster: ClusterName,
    recv_state: Receiver<DroneState>,
    db: DroneDatabase,
) -> NeverResult {
    let mut interval = tokio::time::interval(Duration::from_secs(4));

    loop {
        let state = *recv_state.borrow();
        let ready = state == DroneState::Ready;

        let running_backends = db.running_backends().await?;

        nc.publish(&DroneStatusMessage {
            drone_id: drone_id.clone(),
            cluster: cluster.clone(),
            drone_version: PLANE_VERSION.to_string(),
            ready,
            state,
            running_backends: Some(running_backends as u32),
        })
        .await
        .log_error("Error in ready loop.");

        interval.tick().await;
    }
}

/// Listen for drain instruction.
async fn listen_for_drain(
    nc: TypedNats,
    drone_id: DroneId,
    cluster: ClusterName,
    send_state: Arc<Sender<DroneState>>,
) -> NeverResult {
    let mut sub = nc
        .subscribe(DrainDrone::subscribe_subject(drone_id, cluster))
        .await?;

    while let Some(req) = sub.next().await {
        tracing::info!(req=?req.message(), "Received request to drain drone.");
        req.respond(&()).await?;

        let state = if req.value.drain {
            DroneState::Draining
        } else {
            DroneState::Ready
        };

        send_state
            .send(state)
            .log_error("Error sending drain instruction.");
    }

    Err(anyhow!("Reached the end of DrainDrone subscription."))
}

pub struct AgentSupervisors {
    _ready_loop: Supervisor,
    _listen_for_drain: Supervisor,
    _listen_for_spawn_requests: Supervisor,
    _listen_for_termination_requests: Supervisor,
}

pub async fn run_agent(agent_opts: AgentOptions) -> Result<AgentSupervisors> {
    let nats = &agent_opts.nats;

    tracing::info!("Connecting to Docker.");
    let docker = DockerInterface::try_new(&agent_opts.docker_options).await?;
    tracing::info!("Connecting to sqlite.");
    let db = agent_opts.db;
    let cluster = agent_opts.cluster_domain.clone();
    let ip = do_with_retry(|| agent_opts.ip.get_ip(), 10, Duration::from_secs(10)).await?;

    let request = DroneConnectRequest {
        drone_id: agent_opts.drone_id.clone(),
        cluster: cluster.clone(),
        ip,
        version: Some(PLANE_VERSION.to_string()),
        git_hash: GIT_HASH.map(|s| s.to_string()),
    };

    nats.request(&request).await?;

    let executor = Executor::new(docker, db.clone(), nats.clone(), ip, cluster.clone());

    let (send_state, recv_state) = watch::channel(DroneState::Ready);

    let ready_loop = {
        let nats = nats.clone();
        let cluster = cluster.clone();
        let db = db.clone();
        let drone_id = agent_opts.drone_id.clone();
        move || {
            ready_loop(
                nats.clone(),
                drone_id.clone(),
                cluster.clone(),
                recv_state.clone(),
                db.clone(),
            )
        }
    };

    let listen_for_drain = {
        let nats = nats.clone();
        let cluster = cluster.clone();
        let send_state = Arc::new(send_state);
        let drone_id = agent_opts.drone_id.clone();
        move || {
            listen_for_drain(
                nats.clone(),
                drone_id.clone(),
                cluster.clone(),
                send_state.clone(),
            )
        }
    };

    let listen_for_spawn_requests = {
        let nats = nats.clone();
        let cluster = cluster.clone();
        let executor = executor.clone();
        let drone_id = agent_opts.drone_id.clone();
        move || {
            listen_for_spawn_requests(
                cluster.clone(),
                drone_id.clone(),
                executor.clone(),
                nats.clone(),
            )
        }
    };

    let listen_for_termination_requests = {
        let nats = nats.clone();
        let cluster = cluster.clone();
        move || listen_for_termination_requests(executor.clone(), nats.clone(), cluster.clone())
    };

    Ok(AgentSupervisors {
        _ready_loop: Supervisor::new("ready_loop", ready_loop),
        _listen_for_drain: Supervisor::new("listen_for_drain", listen_for_drain),
        _listen_for_spawn_requests: Supervisor::new(
            "listen_for_spawn_requests",
            listen_for_spawn_requests,
        ),
        _listen_for_termination_requests: Supervisor::new(
            "listen_for_termination_requests",
            listen_for_termination_requests,
        ),
    })
}
