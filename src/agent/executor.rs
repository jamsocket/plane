use super::docker::DockerInterface;
use crate::{
    agent::wait_port_ready,
    database::DroneDatabase,
    messages::agent::{BackendState, BackendStateMessage, SpawnRequest},
    nats::TypedNats,
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use std::{fmt::Debug, net::IpAddr};
use tracing::Level;

pub struct Executor {
    host_ip: IpAddr,
    docker: DockerInterface,
    database: DroneDatabase,
    nc: TypedNats,
}

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

impl Executor {
    pub fn new(
        docker: DockerInterface,
        database: DroneDatabase,
        nc: TypedNats,
        host_ip: IpAddr,
    ) -> Self {
        Executor {
            host_ip,
            docker,
            database,
            nc,
        }
    }

    pub async fn run_backend(&self, spawn_request: &SpawnRequest, mut state: BackendState) {
        let _span = tracing::span!(
            Level::INFO,
            "run_backend",
            backend_id = spawn_request.backend_id.id()
        );
        let _handle = _span.enter();

        // TODO: allow resuming backend.
        self.database
            .insert_backend(spawn_request)
            .await
            .log_error();

        loop {
            tracing::info!(?state, "Executing state.");
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

            let next_state = self.step(spawn_request, state).await;

            match next_state {
                Ok(Some(new_state)) => {
                    state = new_state;

                    continue;
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
                self.docker.pull_image(&spawn_request.image).await?;

                let backend_id = spawn_request.backend_id.id().to_string();
                self.docker
                    .run_container(&backend_id, &spawn_request.image, &spawn_request.env)
                    .await?;

                Ok(Some(BackendState::Starting))
            }
            BackendState::Starting => {
                let port = self
                    .docker
                    .get_port(&spawn_request.backend_id.id())
                    .await
                    .ok_or_else(|| {
                        anyhow!(
                            "Couldn't get port of container {}",
                            spawn_request.backend_id.id()
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
                        .ok_or(anyhow!("Checked add error."))?;

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
                self.docker
                    .stop_container(&spawn_request.backend_id.id())
                    .await?;

                Ok(None)
            }
        }
    }
}
