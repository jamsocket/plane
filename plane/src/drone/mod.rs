use self::{
    executor::Executor,
    heartbeat::HeartbeatLoop,
    key_manager::KeyManager,
    runtime::{docker::DockerRuntimeConfig, Runtime},
    state_store::StateStore,
};
use crate::{
    client::PlaneClient,
    database::backend::BackendActionMessage,
    drone::runtime::docker::DockerRuntime,
    names::DroneName,
    protocol::{BackendAction, MessageFromDrone, MessageToDrone, RenewKeyResponse},
    signals::wait_for_shutdown_signal,
    typed_socket::client::TypedSocketConnector,
    types::{BackendState, ClusterName, DronePoolName},
};
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
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
mod executor;
mod heartbeat;
mod key_manager;
pub mod runtime;
mod state_store;

pub async fn drone_loop<R: Runtime>(
    name: DroneName,
    mut connection: TypedSocketConnector<MessageFromDrone>,
    executor: Executor<R>,
) {
    let executor = Arc::new(executor);
    let key_manager = Arc::new(Mutex::new(KeyManager::new(executor.clone())));

    loop {
        let mut socket = connection.connect_with_retry(&name).await;
        let _heartbeat_guard = HeartbeatLoop::start(socket.sender(MessageFromDrone::Heartbeat));

        {
            let socket = socket.sender(MessageFromDrone::BackendMetrics);
            executor.runtime.metrics_callback(move |metrics_message| {
                if let Err(err) = socket.send(metrics_message) {
                    tracing::error!(?err, "Error sending metrics message.");
                }
            });
        };

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
                        action = ?action,
                        "Received action."
                    );

                    if let BackendAction::Spawn { key, .. } = &action {
                        if key.deadlines.soft_terminate_at.0 < Utc::now() {
                            tracing::warn!(
                                backend_id = backend_id.as_value(),
                                "Received spawn request with deadline in the past. Ignoring."
                            );
                        }

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
                    if let Err(err) = executor.ack_event(event_id) {
                        tracing::error!(?err, "Error acking event.");
                    }
                }
                MessageToDrone::RenewKeyResponse(renew_key_response) => {
                    let RenewKeyResponse { backend, deadlines } = renew_key_response;
                    tracing::info!(
                        backend_id = backend.as_value(),
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
    pub async fn run(config: DroneConfig) -> Result<Self> {
        let client = PlaneClient::new(config.controller_url);

        #[allow(deprecated)]
        let runtime = match (config.docker_config, config.executor_config) {
            (Some(_), Some(_)) => {
                tracing::error!(
                    "Only one of `docker_config` and `executor_config` may be provided."
                );
                return Err(anyhow!(
                    "Only one of `docker_config` and `executor_config` may be provided."
                ));
            }
            (Some(mut docker_config), None) => {
                tracing::warn!("`docker_config` is deprecated. Use `executor_config` instead.");
                docker_config.auto_prune = docker_config.auto_prune.or(config.auto_prune);
                docker_config.cleanup_min_age =
                    docker_config.cleanup_min_age.or(config.cleanup_min_age);

                DockerRuntime::new(docker_config).await?
            }
            (None, Some(ExecutorConfig::Docker(mut docker_config))) => {
                docker_config.auto_prune = docker_config.auto_prune.or(config.auto_prune);
                docker_config.cleanup_min_age =
                    docker_config.cleanup_min_age.or(config.cleanup_min_age);

                DockerRuntime::new(docker_config).await?
            }
            (None, None) => {
                tracing::error!("Neither `docker_config` nor `executor_config` provided.");
                return Err(anyhow!(
                    "Neither `docker_config` nor `executor_config` provided."
                ));
            }
        };

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

        let runtime = Arc::new(runtime);
        let executor = Executor::new(runtime, state_store, config.ip);

        let id = config.name.clone();
        let drone_loop = tokio::spawn(drone_loop(id.clone(), connector, executor));

        Ok(Self { drone_loop, id })
    }

    pub async fn terminate(self) {
        self.drone_loop.abort();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorConfig {
    Docker(DockerRuntimeConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneConfig {
    pub name: DroneName,

    #[deprecated(since = "0.4.12", note = "Use `executor_config` instead.")]
    pub docker_config: Option<DockerRuntimeConfig>,

    // This will become non-optional when docker_config is removed.
    pub executor_config: Option<ExecutorConfig>,

    pub controller_url: Url,
    pub cluster: ClusterName,
    pub pool: DronePoolName,
    pub ip: IpAddr,
    pub db_path: Option<PathBuf>,

    #[deprecated(
        since = "0.4.12",
        note = "Moved to `executor_config` (only applies to DockerRuntimeConfig)."
    )]
    pub auto_prune: Option<bool>,
    #[serde(with = "crate::serialization::serialize_optional_duration_as_seconds")]
    #[deprecated(
        since = "0.4.12",
        note = "Moved to `executor_config` (only applies to DockerRuntimeConfig)."
    )]
    pub cleanup_min_age: Option<Duration>,
}

pub async fn run_drone(config: DroneConfig) -> Result<()> {
    tracing::info!(name=%config.name, "Starting drone");
    let drone = Drone::run(config).await?;

    tracing::info!("Drone started.");
    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down.");

    drone.terminate().await;

    Ok(())
}
