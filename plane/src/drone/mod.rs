use self::{
    docker::PlaneDockerConfig, executor::Executor, heartbeat::HeartbeatLoop,
    key_manager::KeyManager, state_store::StateStore,
};
use crate::{
    client::PlaneClient,
    database::backend::BackendActionMessage,
    drone::docker::PlaneDocker,
    names::DroneName,
    protocol::{BackendAction, MessageFromDrone, MessageToDrone, RenewKeyResponse},
    signals::wait_for_shutdown_signal,
    typed_socket::client::TypedSocketConnector,
    types::{BackendState, ClusterName},
};
use anyhow::Result;
use bollard::Docker;
use chrono::Duration;
use rusqlite::Connection;
use std::{
    fs::{set_permissions, File, Permissions},
    net::IpAddr,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;
use url::Url;
use valuable::Valuable;

mod backend_manager;
pub mod command;
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

        let metrics_sender = socket.sender(MessageFromDrone::BackendMetrics);
        executor.register_metrics_sender(metrics_sender);

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
                if matches!(message.state, BackendState::Terminated { .. }) {
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
                    tracing::info!(
                        backend_id = backend_id.as_value(),
                        action = action.as_value(),
                        "Received action."
                    );

                    if let BackendAction::Spawn { key, .. } = &action {
                        // Register the key with the key manager, ensuring that it will be refreshed.
                        let result = key_manager
                            .lock()
                            .expect("Key manager lock poisoned.")
                            .register_key(backend_id.clone(), key.clone());

                        if !result {
                            tracing::error!(
                                backend = backend_id.as_value(),
                                "Key already registered for backend. Ignoring spawn request."
                            );
                            continue;
                        }
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
                    tracing::info!(event_id = event_id.as_value(), "Received status ack.");
                    if let Err(err) = executor.ack_event(event_id) {
                        tracing::error!(?err, "Error acking event.");
                    }
                }
                MessageToDrone::RenewKeyResponse(renew_key_response) => {
                    let RenewKeyResponse { backend, deadlines } = renew_key_response;
                    tracing::info!(
                        deadlines = deadlines.as_value(),
                        "Received key renewal response."
                    );

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
    pub id: DroneName,
}

impl Drone {
    pub async fn run(config: DronePlan) -> Result<Self> {
        let client = PlaneClient::new(config.controller_url);
        let docker =
            PlaneDocker::new(Docker::connect_with_local_defaults()?, config.docker_config).await?;
        let connector = client.drone_connection(&config.cluster, &config.pool);

        let sqlite_connection = if let Some(db_path) = config.db_path.as_ref() {
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
        let executor = Executor::new(
            docker,
            state_store,
            config.ip,
            config.auto_prune,
            config.cleanup_min_age,
        );

        let id = config.id.clone();
        let drone_loop = tokio::spawn(drone_loop(id.clone(), connector, executor));

        Ok(Self { drone_loop, id })
    }

    pub async fn terminate(self) {
        self.drone_loop.abort();
    }
}

pub struct DronePlan {
    pub id: DroneName,
    pub docker_config: PlaneDockerConfig,
    pub controller_url: Url,
    pub cluster: ClusterName,
    pub ip: IpAddr,
    pub db_path: Option<PathBuf>,
    pub pool: String,
    pub auto_prune: bool,
    pub cleanup_min_age: Duration,
}

pub async fn run_drone(plan: DronePlan) -> Result<()> {
    let drone = Drone::run(plan).await?;

    tracing::info!("Drone started.");
    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down.");

    drone.terminate().await;

    Ok(())
}
