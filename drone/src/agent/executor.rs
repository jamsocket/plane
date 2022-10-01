use super::{
    backend::BackendMonitor,
    docker::{ContainerEventType, DockerInterface},
};
use crate::{
    agent::wait_port_ready,
    database::{Backend, DroneDatabase},
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use dashmap::DashMap;
use dis_spawner::{
    messages::agent::{BackendState, BackendStateMessage, SpawnRequest, TerminationRequest},
    nats::TypedNats,
    types::{BackendId, ClusterName},
};
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

#[derive(Clone)]
pub struct Executor {
    docker: DockerInterface,
    database: DroneDatabase,
    nc: TypedNats,
    _container_events_handle: Arc<JoinHandle<()>>,
    backend_to_monitor: Arc<DashMap<BackendId, BackendMonitor>>,
    backend_to_listener: Arc<DashMap<BackendId, Sender<()>>>,
    ip: IpAddr,
    cluster: ClusterName,
}

impl Executor {
    pub fn new(
        docker: DockerInterface,
        database: DroneDatabase,
        nc: TypedNats,
        ip: IpAddr,
        cluster: ClusterName,
    ) -> Self {
        let backend_to_listener: Arc<DashMap<BackendId, Sender<()>>> = Arc::default();
        let container_events_handle = tokio::spawn(Self::listen_for_container_events(
            docker.clone(),
            backend_to_listener.clone(),
        ));

        Executor {
            docker,
            database,
            nc,
            _container_events_handle: Arc::new(container_events_handle),
            backend_to_monitor: Arc::default(),
            backend_to_listener,
            ip,
            cluster,
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
            .publish_jetstream(&BackendStateMessage::new(
                BackendState::Loading,
                spawn_request.backend_id.clone(),
            ))
            .await
            .log_error();

        self.run_backend(spawn_request, BackendState::Loading).await
    }

    pub async fn kill_backend(
        &self,
        termination_request: &TerminationRequest,
    ) -> Result<(), anyhow::Error> {
        tracing::info!(backend_id=%termination_request.backend_id, "Trying to terminate backend");
        self.docker
            .stop_container(&termination_request.backend_id.to_resource_name())
            .await
    }

    pub async fn resume_backends(&self) -> Result<()> {
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
                self.backend_to_monitor.insert(
                    backend_id.clone(),
                    BackendMonitor::new(
                        &backend_id,
                        &self.cluster,
                        self.ip,
                        &self.docker,
                        &self.nc,
                    ),
                );
            }
            tokio::spawn(async move { executor.run_backend(&spec, state).await });
        }

        Ok(())
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
                        self.backend_to_monitor.insert(
                            spawn_request.backend_id.clone(),
                            BackendMonitor::new(
                                &spawn_request.backend_id,
                                &self.cluster,
                                self.ip,
                                &self.docker,
                                &self.nc,
                            ),
                        );
                    }

                    self.database
                        .update_backend_state(&spawn_request.backend_id, state)
                        .await
                        .log_error();
                    self.nc
                        .publish_jetstream(&BackendStateMessage::new(
                            state,
                            spawn_request.backend_id.clone(),
                        ))
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
                    .run_container(
                        &backend_id,
                        &spawn_request.image,
                        &spawn_request.env,
                        &spawn_request.resource_limits,
                    )
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

                let container_ip = self
                    .docker
                    .get_ip(&spawn_request.backend_id.to_resource_name())
                    .await
                    .unwrap();

                tracing::info!(%container_ip, "Got IP from container.");
                wait_port_ready(8080, container_ip).await?;

                self.database
                    .insert_proxy_route(
                        &spawn_request.backend_id,
                        spawn_request.backend_id.id(),
                        &format!("{}:{}", container_ip, 8080),
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

                self.docker
                    .stop_container(&container_name)
                    .await
                    .map_err(|e| anyhow!("Error stopping container: {:?}", e))?;

                Ok(None)
            }
        }
    }
}
