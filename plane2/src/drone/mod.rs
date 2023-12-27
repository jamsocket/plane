use self::{
    executor::Executor, heartbeat::HeartbeatLoop, key_manager::KeyManager, state_store::StateStore,
};
use crate::{
    client::PlaneClient,
    database::backend::BackendActionMessage,
    drone::docker::PlaneDocker,
    names::DroneName,
    protocol::{BackendAction, MessageFromDrone, MessageToDrone, RenewKeyResponse},
    signals::wait_for_shutdown_signal,
    typed_socket::client::TypedSocketConnector,
    types::{BackendStatus, ClusterName},
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
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;

mod backend_manager;
pub mod docker;
mod executor;
mod heartbeat;
mod key_manager;
mod state_store;
mod wait_backend;

pub async fn drone_loop(
    name: DroneName,
    mut connection: TypedSocketConnector<MessageFromDrone>,
    executor: Executor,
) {
    let executor = Arc::new(executor);
    let key_manager = Arc::new(Mutex::new(KeyManager::new(executor.clone())));

    loop {
        let mut socket = connection.connect_with_retry(&name).await;
        let _heartbeat_guard = HeartbeatLoop::start(socket.sender(MessageFromDrone::Heartbeat));

        key_manager
            .lock()
            .expect("Key manager lock poisoned")
            .set_sender(socket.sender(MessageFromDrone::RenewKey));

        {
            // Forward state changes to the socket.
            // This will start by sending any existing unacked events.
            let sender = socket.sender(MessageFromDrone::BackendEvent);
            let key_manager = key_manager.clone();
            if let Err(err) = executor.register_listener(move |message| {
                if message.status == BackendStatus::Terminated {
                    key_manager
                        .lock()
                        .expect("Key manager lock poisoned.")
                        .unregister_key(&message.backend_id);
                }

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

                    if let BackendAction::Spawn { key, .. } = &action {
                        // Register the key with the key manager, ensuring that it will be refreshed.
                        key_manager
                            .lock()
                            .expect("Key manager lock poisoned.")
                            .register_key(backend_id.clone(), key.clone());
                    }

                    if let Err(err) = executor.apply_action(&backend_id, &action).await {
                        tracing::error!(?err, "Error applying action.");
                        continue;
                    }

                    if let Err(err) = socket.send(MessageFromDrone::AckAction { action_id }).await {
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
                MessageToDrone::RenewKeyResponse(renew_key_response) => {
                    let RenewKeyResponse { backend, deadlines } = renew_key_response;
                    tracing::info!(?deadlines, "Received key renewal response.");

                    if let Some(deadlines) = deadlines {
                        key_manager
                            .lock()
                            .expect("Key manager lock poisoned.")
                            .update_deadlines(&backend, deadlines);
                    } else {
                        // TODO: we could begin the graceful termiation here.
                        tracing::warn!("Key renewal failed.");
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
        docker_runtime: Option<String>,
    ) -> Result<Self> {
        // Wait until we have loaded initial state from Docker to begin.
        let docker = PlaneDocker::new(docker, docker_runtime).await?;

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
    cluster: ClusterName,
    ip: IpAddr,
    db_path: Option<&Path>,
    docker_runtime: Option<String>,
) -> Result<()> {
    let connection = client.drone_connection(&cluster);

    let ip = if let Some(ip) = get_internal_host_ip() {
        tracing::info!(%ip, "Found internal host IP.");
        ip
    } else {
        tracing::warn!("Could not find internal host IP.");
        ip
    };

    let drone = Drone::run(&id, connection, docker, ip, db_path, docker_runtime).await?;

    tracing::info!("Drone started.");
    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down.");

    drone.terminate().await;

    Ok(())
}
