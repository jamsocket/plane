use super::docker::{ContainerEventType, DockerInterface};
use crate::{
    database::DroneDatabase,
    drone::agent::wait_port_ready,
    messages::agent::{BackendState, BackendStateMessage, SpawnRequest},
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
use tracing::Level;

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
        }
    }

    pub async fn listen_for_container_events(
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

    pub async fn run_backend(&self, spawn_request: &SpawnRequest, mut state: BackendState) {
        let _span = tracing::span!(
            Level::INFO,
            "run_backend",
            backend_id = spawn_request.backend_id.id(),
            metadata = %json!(spawn_request.metadata),
        );
        let _handle = _span.enter();

        // TODO: allow resuming backend.
        self.database
            .insert_backend(spawn_request)
            .await
            .log_error();

        let (send, mut recv) = channel(1);
        self.backend_to_listener
            .insert(spawn_request.backend_id.clone(), send);

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

        loop {
            tracing::info!(?state, "Executing state.");

            let next_state = tokio::select! {
                next_state = self.step(spawn_request, state) => next_state,
                _ = recv.recv() => {
                    tracing::info!("State may have updated externally.");
                    continue;
                },
            };

            match next_state {
                Ok(Some(new_state)) => {
                    state = new_state;
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
                            spawn_request.max_idle_time,
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
