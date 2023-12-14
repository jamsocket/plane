use self::{executor::Executor, heartbeat::HeartbeatLoop, state_store::StateStore};
use crate::{
    client::PlaneClient,
    database::backend::BackendActionMessage,
    drone::docker::PlaneDocker,
    names::DroneName,
    protocol::{MessageFromDrone, MessageToDrone},
    signals::wait_for_shutdown_signal,
    typed_socket::{client::TypedSocketConnector, FullDuplexChannel},
    types::ClusterId,
    util::get_internal_host_ip,
};
use anyhow::Result;
use bollard::Docker;
use rusqlite::Connection;
use std::{
    fs::{set_permissions, File, Permissions},
    net::IpAddr,
    os::unix::fs::PermissionsExt,
    path::Path,
};
use tokio::task::JoinHandle;

mod backend_manager;
pub mod docker;
mod executor;
mod heartbeat;
mod state_store;
mod wait_backend;

pub async fn drone_loop(
    name: DroneName,
    mut connection: TypedSocketConnector<MessageFromDrone>,
    mut executor: Executor,
) {
    loop {
        let mut socket = connection.connect_with_retry(&name).await;
        let _heartbeat_guard = HeartbeatLoop::start(socket.sender());

        {
            // Forward state changes to the socket.
            // This will start by sending any existing unacked events.
            let sender = socket.sender();
            if let Err(err) = executor.register_listener(move |message| {
                let message = MessageFromDrone::BackendEvent(message);
                if let Err(e) = sender.send(message) {
                    tracing::error!(%e, "Error sending message.");
                }
            }) {
                tracing::error!(?err, "Error registering listener.");
                continue;
            }
        }

        loop {
            let Some(message) = socket.recv().await else {
                tracing::warn!("Connection closed.");
                break;
            };

            match message {
                MessageToDrone::Action(BackendActionMessage {
                    action_id,
                    backend_id,
                    action,
                    ..
                }) => {
                    tracing::info!(?backend_id, ?action, "Received action.");

                    if let Err(err) = executor.apply_action(&backend_id, &action).await {
                        tracing::error!(?err, "Error applying action.");
                        continue;
                    }

                    if let Err(err) = socket
                        .send(&MessageFromDrone::AckAction { action_id })
                        .await
                    {
                        tracing::error!(?err, "Error sending ack.");
                        continue;
                    }
                }
                MessageToDrone::AckEvent { event_id } => {
                    tracing::info!(?event_id, "Received status ack.");
                    if let Err(err) = executor.ack_event(event_id) {
                        tracing::error!(?err, "Error acking event.");
                    }
                }
            }
        }
    }
}

pub struct Drone {
    drone_loop: JoinHandle<()>,
}

impl Drone {
    pub async fn run(
        id: &DroneName,
        connector: TypedSocketConnector<MessageFromDrone>,
        docker: Docker,
        ip: IpAddr,
        db_path: Option<&Path>,
    ) -> Result<Self> {
        // Wait until we have loaded initial state from Docker to begin.
        let docker = PlaneDocker::new(docker).await?;

        let sqlite_connection = if let Some(db_path) = db_path {
            if !db_path.exists() {
                File::create(db_path)?;
                let permissions = Permissions::from_mode(0o600);
                set_permissions(db_path, permissions)?;
            }

            Connection::open(db_path)?
        } else {
            Connection::open_in_memory()?
        };

        let state_store = StateStore::new(sqlite_connection)?;
        let executor = Executor::new(docker, state_store, ip);

        let id = id.clone();
        let drone_loop = tokio::spawn(drone_loop(id, connector, executor));

        Ok(Self { drone_loop })
    }

    pub async fn terminate(self) {
        self.drone_loop.abort();
    }
}

pub async fn run_drone(
    client: PlaneClient,
    docker: Docker,
    id: DroneName,
    cluster: ClusterId,
    ip: IpAddr,
    db_path: Option<&Path>,
) -> Result<()> {
    let connection = client.drone_connection(&cluster);

    let ip = if let Some(ip) = get_internal_host_ip() {
        tracing::info!(%ip, "Found internal host IP.");
        ip
    } else {
        tracing::warn!("Could not find internal host IP.");
        ip
    };

    let drone = Drone::run(&id, connection, docker, ip, db_path).await?;

    tracing::info!("Drone started.");
    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down.");

    drone.terminate().await;

    Ok(())
}
