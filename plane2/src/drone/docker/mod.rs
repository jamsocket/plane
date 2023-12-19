use self::{
    commands::{get_port, run_container},
    types::ContainerId,
};
use crate::{names::BackendName, types::ExecutorConfig};
use anyhow::Result;
use bollard::{errors::Error, service::EventMessage, system::EventsOptions, Docker, container::StatsOptions, auth::DockerCredentials};
use tokio_stream::{Stream, StreamExt};

mod commands;
pub mod types;

/// The label used to identify containers managed by Plane.
/// The existence of this label is used to determine whether a container is managed by Plane.
const PLANE_DOCKER_LABEL: &str = "dev.plane.backend";

#[derive(Clone)]
pub struct PlaneDocker {
    docker: Docker,
    runtime: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TerminateEvent {
    pub backend_id: BackendName,
    pub exit_code: Option<i32>,
}

pub struct SpawnResult {
    pub container_id: ContainerId,
    pub port: u16,
}

impl PlaneDocker {
    pub async fn new(docker: Docker, runtime: Option<String>) -> Result<Self> {
        Ok(Self { docker, runtime })
    }

    pub async fn pull(
        &self,
        image: &str,
        credentials: Option<&DockerCredentials>,
        force: bool,
    ) -> Result<()> {
        commands::pull_image(&self.docker, image, credentials, force).await?;
        Ok(())
    }

    pub async fn backend_events(&self) -> impl Stream<Item = TerminateEvent> {
        let options = EventsOptions {
            since: None,
            until: None,
            filters: vec![
                ("type", vec!["container"]),
                ("event", vec!["die", "stop"]),
                ("label", vec![PLANE_DOCKER_LABEL]),
            ]
            .into_iter()
            .collect(),
        };
        self.docker.events(Some(options)).filter_map(|e| {
            let e: EventMessage = match e {
                Err(e) => {
                    tracing::error!(?e, "Error receiving Docker event.");
                    return None;
                }
                Ok(e) => e,
            };

            tracing::info!("Received event: {:?}", e);

            let Some(actor) = e.actor else {
                tracing::warn!("Received event without actor.");
                return None;
            };
            let Some(attributes) = actor.attributes else {
                tracing::warn!("Received event without attributes.");
                return None;
            };

            let exit_code = attributes.get("exitCode");
            let exit_code = exit_code.and_then(|s| s.parse::<i32>().ok());
            let Some(backend_id) = attributes.get(PLANE_DOCKER_LABEL) else {
                tracing::warn!(
                    "Ignoring event without Plane backend \
                    ID (this is expected if non-Plane \
                    backends are running on the same Docker instance.)"
                );
                return None;
            };
            let backend_id = match BackendName::try_from(backend_id.to_string()) {
                Ok(backend_id) => backend_id,
                Err(err) => {
                    tracing::warn!(?err, backend_id, "Ignoring event with invalid backend ID.");
                    return None;
                }
            };

            tracing::info!("Received exit code: {:?}", exit_code);

            Some(TerminateEvent {
                backend_id,
                exit_code,
            })
        })
    }

    pub async fn spawn_backend(
        &self,
        backend_id: &BackendName,
        container_id: &ContainerId,
        executable: ExecutorConfig,
    ) -> Result<SpawnResult> {
        run_container(
            &self.docker,
            backend_id,
            container_id,
            executable,
            &self.runtime,
        )
        .await?;
        let port = get_port(&self.docker, container_id).await?;

        Ok(SpawnResult {
            container_id: container_id.clone(),
            port,
        })
    }

    pub async fn terminate_backend(
        &self,
        container_id: &ContainerId,
        hard: bool,
    ) -> Result<(), Error> {
        if hard {
            self.docker
                .kill_container::<String>(&container_id.to_string(), None)
                .await?;
        } else {
            self.docker
                .stop_container(&container_id.to_string(), None)
                .await?;
        }

        Ok(())
    }

	pub async fn get_metrics(&self, container_id: &ContainerId) -> Result<bollard::container::Stats> {
		let options = StatsOptions {
			stream: false,
			one_shot: false
		};


		self.docker.stats(&container_id.to_string(), Some(options)).next().await.ok_or(anyhow::anyhow!("noooo"))?.map_err(|_| anyhow::anyhow!("welp"))
	}
}
