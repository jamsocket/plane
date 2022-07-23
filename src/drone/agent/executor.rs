use super::docker::{ContainerEventType, DockerInterface};
use crate::{
    database::{Backend, DroneDatabase},
    drone::agent::wait_port_ready,
    messages::agent::{
        BackendState, BackendStateMessage, DroneLogMessage, DroneStatsMessage, SpawnRequest,
    },
    nats::TypedNats,
    types::BackendId,
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use dashmap::DashMap;
use serde_json::json;
use std::{fmt::Debug, net::IpAddr, sync::Arc};
use tokio::{
    sync::mpsc::{channel, Sender},
    task::JoinHandle,
};
use tokio_stream::StreamExt;

trait LogError {
    fn log_error(&self) -> &Self;
}

impl<T, E: Debug> LogError for Result<T, E> {
    fn log_error(&self) -> &Self {
        match self {
            Ok(_) => (),
            Err(error) => tracing::error!(?error, "Encountered non-blocking error."),
        }

        self
    }
}

pub struct Executor {
    host_ip: IpAddr,
    docker: DockerInterface,
    database: DroneDatabase,
    nc: TypedNats,
    _container_events_handle: JoinHandle<()>,
    backend_to_listener: Arc<DashMap<BackendId, Sender<()>>>,
    backend_to_log_loop:
        Arc<DashMap<BackendId, tokio::task::JoinHandle<Result<(), anyhow::Error>>>>,
    backend_to_stats_loop:
        Arc<DashMap<BackendId, tokio::task::JoinHandle<Result<(), anyhow::Error>>>>,
}

impl Executor {
    pub fn new(
        docker: DockerInterface,
        database: DroneDatabase,
        nc: TypedNats,
        host_ip: IpAddr,
    ) -> Self {
        let backend_to_listener: Arc<DashMap<BackendId, Sender<()>>> = Arc::default();
        let container_events_handle = tokio::spawn(Self::listen_for_container_events(
            docker.clone(),
            backend_to_listener.clone(),
        ));

        Executor {
            host_ip,
            docker,
            database,
            nc,
            _container_events_handle: container_events_handle,
            backend_to_listener,
            backend_to_log_loop: Arc::default(),
            backend_to_stats_loop: Arc::default(),
        }
    }

    async fn listen_for_container_events(
        docker: DockerInterface,
        backend_to_listener: Arc<DashMap<BackendId, Sender<()>>>,
    ) {
        let mut event_stream = docker.container_events().await;
        while let Some(event) = event_stream.next().await {
            if event.event == ContainerEventType::Die {
                let backend_id =
                    if let Some(backend_id) = BackendId::from_resource_name(&event.name) {
                        backend_id
                    } else {
                        continue;
                    };

                if let Some(v) = backend_to_listener.get(&backend_id) {
                    v.try_send(()).log_error();
                }
            }
        }
    }

    pub async fn start_backend(&self, spawn_request: &SpawnRequest) {
        self.database
            .insert_backend(spawn_request)
            .await
            .log_error();

        self.nc
            .publish(
                &BackendStateMessage::subject(&spawn_request.backend_id),
                &BackendStateMessage::new(BackendState::Loading),
            )
            .await
            .log_error();

        self.run_backend(spawn_request, BackendState::Loading).await
    }

    pub async fn resume_backends(self: &Arc<Self>) -> Result<()> {
        let backends = self.database.get_backends().await?;

        for backend in backends {
            let executor = self.clone();
            let Backend {
                backend_id,
                state,
                spec,
            } = backend;
            tracing::info!(%backend_id, ?state, "Resuming backend");

            if state.running() {
                self.start_log_loop(&backend_id);
                self.start_stats_loop(&backend_id);
            }
            tokio::spawn(async move { executor.run_backend(&spec, state).await });
        }

        Ok(())
    }

    fn start_log_loop(&self, backend_id: &BackendId) {
        let docker = self.docker.clone();
        let nc = self.nc.clone();
        let backend_id = backend_id.clone();
        self.backend_to_log_loop
            .entry(backend_id.clone())
            .or_insert_with(move || {
                tokio::spawn(async move {
                    let container_name = backend_id.to_resource_name();
                    tracing::info!(%backend_id, "Log recording loop started.");
                    let mut stream = docker.get_logs(&container_name);

                    while let Some(v) = stream.next().await {
                        match v {
                            Ok(v) => {
                                if let Some(message) = DroneLogMessage::from_log_message(&v) {
                                    nc.publish(&DroneLogMessage::subject(&backend_id), &message)
                                        .await?;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(?error, "Error encountered forwarding log.");
                            }
                        }
                    }

                    tracing::info!(%backend_id, "Log loop terminated.");

                    Ok::<(), anyhow::Error>(())
                })
            });
    }

    fn start_stats_loop(&self, backend_id: &BackendId) {
        let docker = self.docker.clone();
        let nc = self.nc.clone();
        let backend_id = backend_id.clone();
        self.backend_to_stats_loop
            .entry(backend_id.clone())
            .or_insert((|| {
                return tokio::spawn(async move {
                    let container_name = backend_id.to_resource_name();
                    tracing::info!(%backend_id, "Stats recording loop started.");
                    let mut stream = Box::pin(docker.get_stats(&container_name));

                    while let Some(v) = stream.next().await {
                        match v {
                            Ok(v) => {
                                if let Some(message) = DroneStatsMessage::from_stats_message(&v) {
                                    nc.publish(&DroneStatsMessage::subject(&backend_id), &message)
                                        .await?;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(?error, "Error encountered sending stats.")
                            }
                        }
                    }

                    Ok(())
                });
            })());
    }

    async fn run_backend(&self, spawn_request: &SpawnRequest, mut state: BackendState) {
        let (send, mut recv) = channel(1);
        self.backend_to_listener
            .insert(spawn_request.backend_id.clone(), send);

        loop {
            tracing::info!(
                ?state,
                backend_id = spawn_request.backend_id.id(),
                metadata = %json!(spawn_request.metadata),
                "Executing state."
            );

            let next_state = loop {
                if state == BackendState::Swept {
                    // When sweeping, we ignore external state changes to avoid an infinite loop.
                    break self.step(spawn_request, state).await;
                } else {
                    // Otherwise, we allow the step to be interrupted if the state changes (i.e.
                    // if the container dies).
                    tokio::select! {
                        next_state = self.step(spawn_request, state) => break next_state,
                        _ = recv.recv() => {
                            tracing::info!("State may have updated externally.");
                            continue;
                        },
                    }
                };
            };

            match next_state {
                Ok(Some(new_state)) => {
                    state = new_state;

                    if state.running() {
                        self.start_log_loop(&spawn_request.backend_id);
                        self.start_stats_loop(&spawn_request.backend_id)
                    }

                    self.database
                        .update_backend_state(&spawn_request.backend_id, state)
                        .await
                        .log_error();
                    self.nc
                        .publish(
                            &BackendStateMessage::subject(&spawn_request.backend_id),
                            &BackendStateMessage::new(state),
                        )
                        .await
                        .log_error();
                }
                Ok(None) => {
                    // Successful termination.
                    tracing::info!("Terminated successfully.");
                    break;
                }
                Err(error) => {
                    tracing::error!(?error, ?state, "Encountered error.");
                    break;
                }
            }
        }
    }

    pub async fn step(
        &self,
        spawn_request: &SpawnRequest,
        state: BackendState,
    ) -> Result<Option<BackendState>> {
        match state {
            BackendState::Loading => {
                self.docker
                    .pull_image(&spawn_request.image, &spawn_request.credentials)
                    .await?;

                let backend_id = spawn_request.backend_id.to_resource_name();
                self.docker
                    .run_container(&backend_id, &spawn_request.image, &spawn_request.env)
                    .await?;
                tracing::info!(%backend_id, "Container is running.");

                Ok(Some(BackendState::Starting))
            }
            BackendState::Starting => {
                if !self
                    .docker
                    .is_running(&spawn_request.backend_id.to_resource_name())
                    .await?
                    .0
                {
                    return Ok(Some(BackendState::ErrorStarting));
                }

                let port = self
                    .docker
                    .get_port(&spawn_request.backend_id.to_resource_name())
                    .await
                    .ok_or_else(|| {
                        anyhow!(
                            "Couldn't get port of container {}",
                            spawn_request.backend_id.to_resource_name()
                        )
                    })?;

                tracing::info!(%port, "Got port from container.");
                wait_port_ready(port, self.host_ip).await?;

                self.database
                    .insert_proxy_route(
                        &spawn_request.backend_id,
                        spawn_request.backend_id.id(),
                        &format!("{}:{}", self.host_ip, port),
                    )
                    .await?;

                Ok(Some(BackendState::Ready))
            }
            BackendState::Ready => {
                if let (false, exit_code) = self
                    .docker
                    .is_running(&spawn_request.backend_id.to_resource_name())
                    .await?
                {
                    if exit_code == Some(0) {
                        return Ok(Some(BackendState::Exited));
                    } else {
                        return Ok(Some(BackendState::Failed));
                    }
                }

                // wait for idle
                loop {
                    let last_active = self
                        .database
                        .get_backend_last_active(&spawn_request.backend_id)
                        .await?;
                    let next_check = last_active
                        .checked_add_signed(chrono::Duration::from_std(
                            spawn_request.max_idle_secs,
                        )?)
                        .ok_or_else(|| anyhow!("Checked add error."))?;

                    if next_check < Utc::now() {
                        break;
                    } else {
                        tokio::time::sleep(next_check.signed_duration_since(Utc::now()).to_std()?)
                            .await;
                    }
                }

                Ok(Some(BackendState::Swept))
            }
            BackendState::ErrorLoading
            | BackendState::ErrorStarting
            | BackendState::TimedOutBeforeReady
            | BackendState::Failed
            | BackendState::Exited
            | BackendState::Swept => {
                let container_name = spawn_request.backend_id.to_resource_name();
                if self.docker.is_running(&container_name).await?.0 {
                    self.docker
                        .stop_container(&container_name)
                        .await
                        .map_err(|e| anyhow!("Error stopping container: {:?}", e))?;
                }

                Ok(None)
            }
        }
    }
}
