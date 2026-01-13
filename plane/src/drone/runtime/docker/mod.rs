use self::{
    commands::{get_port, run_container},
    types::ContainerId,
    wait_backend::wait_for_backend,
};
use crate::{
    drone::runtime::{docker::metrics::metrics_loop, Runtime},
    heartbeat_consts::KILL_AFTER_SOFT_TERMINATE_SECONDS,
    util::GuardHandle,
};
use anyhow::Result;
use bollard::{
    container::{PruneContainersOptions, StopContainerOptions},
    image::PruneImagesOptions,
    service::{EventMessage, HostConfigLogConfig},
    system::EventsOptions,
    Docker,
};
use chrono::{DateTime, Duration, Utc};
use plane_common::{
    names::BackendName,
    protocol::{AcquiredKey, BackendMetricsMessage},
    types::{backend_state::BackendError, BearerToken, DockerExecutorConfig, PullPolicy},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf, pin::Pin};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast::Sender;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use valuable::Valuable;

/// Clean up containers and images every minute.
const CLEANUP_INTERVAL_SECS: i64 = 60;

pub mod commands;
pub mod metrics;
pub mod types;
mod wait_backend;

/// The label used to identify containers managed by Plane.
/// The existence of this label is used to determine whether a container is managed by Plane.
pub const PLANE_DOCKER_LABEL: &str = "dev.plane.backend";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DockerRuntimeConfig {
    pub runtime: Option<String>,
    pub log_config: Option<HostConfigLogConfig>,
    pub mount_base: Option<PathBuf>,

    pub auto_prune: Option<bool>,

    #[serde(default)] // Necessary because we use a custom deserializer; see https://stackoverflow.com/a/44303505
    #[serde(with = "plane_common::serialization::serialize_optional_duration_as_seconds")]
    pub cleanup_min_age: Option<Duration>,
}

pub type MetricsCallback = Box<dyn Fn(BackendMetricsMessage) + Send + Sync + 'static>;

pub struct DockerRuntime {
    pub docker: Docker,
    config: DockerRuntimeConfig,
    metrics_callback: Arc<Mutex<Option<MetricsCallback>>>,
    events_sender: Sender<TerminateEvent>,
    _events_loop_handle: GuardHandle,
    _cleanup_handle: GuardHandle,
}

async fn events_loop(
    docker: Docker,
    metrics_callback: Arc<Mutex<Option<MetricsCallback>>>,
    event_sender: Sender<TerminateEvent>,
) {
    let options = EventsOptions {
        since: None,
        until: None,
        filters: vec![
            ("type", vec!["container"]),
            ("event", vec!["die", "stop", "start"]),
            ("label", vec![PLANE_DOCKER_LABEL]),
        ]
        .into_iter()
        .collect(),
    };
    let mut stream = docker.events(Some(options));

    while let Some(e) = stream.next().await {
        let e: EventMessage = match e {
            Err(e) => {
                tracing::error!(?e, "Error receiving Docker event.");
                continue;
            }
            Ok(e) => e,
        };

        tracing::debug!(event=?e, "Received event");

        let Some(actor) = &e.actor else {
            tracing::warn!("Received event without actor.");
            continue;
        };
        let Some(attributes) = &actor.attributes else {
            tracing::warn!("Received event without attributes.");
            continue;
        };
        if !attributes.contains_key(PLANE_DOCKER_LABEL) {
            tracing::warn!(?e.actor, "Ignoring event without Plane backend ID label.");
            continue;
        };
        let Some(container_id) = attributes.get("name").cloned().map(ContainerId::from) else {
            tracing::warn!(?e.actor, "Ignoring event without name attribute.");
            continue;
        };
        let Ok(backend_id) = BackendName::try_from(container_id.clone()) else {
            tracing::warn!(?e.actor, ?container_id, "Ignoring event with invalid backend ID.");
            continue;
        };

        if e.action.as_deref() == Some("start") {
            tracing::debug!(?backend_id, "Received start event.");

            let docker = docker.clone();
            let metrics_callback = metrics_callback.clone();
            tracing::debug!(%backend_id, "Spawning metrics loop.");
            tokio::spawn(async move {
                metrics_loop(backend_id, docker, metrics_callback).await;
            });

            continue;
        }

        // By elimination, we know that the event is a stop/die event.

        let exit_code = attributes.get("exitCode");
        let exit_code = exit_code.and_then(|s| s.parse::<i32>().ok());

        tracing::debug!(
            exit_code,
            backend_id = backend_id.as_value(),
            "Received exit code"
        );

        if let Err(err) = event_sender.send(TerminateEvent {
            backend_id,
            exit_code,
        }) {
            tracing::error!(?err, "Error sending event.");
        }
    }
}

#[async_trait::async_trait]
impl Runtime for DockerRuntime {
    async fn prepare(&self, config: &serde_json::Value) -> Result<()> {
        let config: DockerExecutorConfig = serde_json::from_value(config.clone())?;
        let image = &config.image;
        let credentials = config
            .credentials
            .as_ref()
            .map(|credentials| credentials.clone().into());
        let force = match config.pull_policy.unwrap_or_default() {
            PullPolicy::IfNotPresent => false,
            PullPolicy::Always => true,
            PullPolicy::Never => {
                // Skip the loading step.
                return Ok(());
            }
        };

        commands::pull_image(&self.docker, image, credentials.as_ref(), force).await?;
        Ok(())
    }

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: &serde_json::Value,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult> {
        let executable: DockerExecutorConfig = serde_json::from_value(executable.clone())?;
        let container_id =
            run_container(self, backend_id, executable, acquired_key, static_token).await?;
        let port = get_port(&self.docker, &container_id).await?;

        Ok(SpawnResult {
            container_id: container_id.clone(),
            port,
        })
    }

    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<bool, anyhow::Error> {
        let container_id: ContainerId = backend_id.into();

        // check if container is no longer running, since stop_container() returns Ok(()) even when the container is already gone
        match self
            .docker
            .inspect_container(&container_id.to_string(), None)
            .await
        {
            Ok(details) => {
                if !details.state.and_then(|s| s.running).unwrap_or(false) {
                    tracing::warn!(
                        %container_id,
                        %backend_id,
                        "Container could not be terminated, because it is not running."
                    );
                    return Ok(false);
                }
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                tracing::warn!(
                    %container_id,
                    %backend_id,
                    "Container not found, assuming it was already terminated."
                );
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };

        let result = if hard {
            self.docker
                .kill_container::<String>(&container_id.to_string(), None)
                .await
        } else {
            self.docker
                .stop_container(
                    &container_id.to_string(),
                    Some(StopContainerOptions {
                        t: KILL_AFTER_SOFT_TERMINATE_SECONDS,
                    }),
                )
                .await
        };

        match result {
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                tracing::warn!(
                    %container_id,
                    %backend_id,
                    "Container could not be terminated, because it already was."
                );
                Ok(false)
            }
            Err(e) => Err(e.into()),
            Ok(_) => Ok(true),
        }
    }

    fn events(&self) -> Pin<Box<dyn Stream<Item = TerminateEvent> + Send>> {
        Box::pin(
            BroadcastStream::new(self.events_sender.subscribe()).filter_map(|e| match e {
                Ok(e) => Some(e),
                Err(e) => {
                    tracing::error!(?e, "Error receiving Docker event.");
                    None
                }
            }),
        )
    }

    fn metrics_callback(&self, sender: Box<dyn Fn(BackendMetricsMessage) + Send + Sync + 'static>) {
        let mut lock = self
            .metrics_callback
            .lock()
            .expect("Metrics callback lock poisoned.");
        *lock = Some(Box::new(sender));
    }

    async fn wait_for_backend(
        &self,
        _backend: &BackendName,
        address: SocketAddr,
    ) -> Result<(), BackendError> {
        wait_for_backend(address).await
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminateEvent {
    pub backend_id: BackendName,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpawnResult {
    pub container_id: ContainerId,
    pub port: u16,
}

impl DockerRuntime {
    pub async fn new(config: DockerRuntimeConfig) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()?;
        let (events_sender, _) = tokio::sync::broadcast::channel::<TerminateEvent>(1024);

        let cleanup_handle = {
            let docker = docker.clone();
            let cleanup_min_age = config.cleanup_min_age.unwrap_or_default();
            let auto_prune = config.auto_prune.unwrap_or_default();
            GuardHandle::new(async move {
                cleanup_loop(
                    docker.clone(),
                    cleanup_min_age,
                    Duration::try_seconds(CLEANUP_INTERVAL_SECS).expect("duration is always valid"),
                    auto_prune,
                )
                .await;
            })
        };

        let metrics_callback = Arc::new(Mutex::new(None));

        let event_loop_handle = {
            let metrics_callback = metrics_callback.clone();
            let docker = docker.clone();
            let events_sender = events_sender.clone();
            GuardHandle::new(async move {
                events_loop(docker.clone(), metrics_callback.clone(), events_sender).await;
            })
        };

        Ok(Self {
            docker,
            config,
            metrics_callback,
            events_sender,
            _events_loop_handle: event_loop_handle,
            _cleanup_handle: cleanup_handle,
        })
    }
}

async fn cleanup_loop(docker: Docker, min_age: Duration, interval: Duration, auto_prune: bool) {
    loop {
        tokio::time::sleep(
            interval
                .to_std()
                .expect("Expected interval to convert to std."),
        )
        .await;

        let since = Utc::now() - min_age;

        if let Err(e) = prune(&docker, since, auto_prune).await {
            tracing::error!(?e, "Error pruning Docker containers and images.");
        }
    }
}

/// Prune stopped backend containers that are older than the prune threshold.
/// Then, (optionally) remove unused images also older than the prune threshold.
pub async fn prune(docker: &Docker, until: DateTime<Utc>, prune_images: bool) -> Result<()> {
    tracing::info!("Pruning Docker containers and images.");

    let since_unixtime = until.timestamp();
    let filters: HashMap<String, Vec<String>> = vec![
        ("until".to_string(), vec![since_unixtime.to_string()]),
        ("label".to_string(), vec![PLANE_DOCKER_LABEL.to_string()]),
    ]
    .into_iter()
    .collect();

    match docker
        .prune_containers(Some(PruneContainersOptions {
            filters: filters.clone(),
        }))
        .await
    {
        Ok(result) => {
            let num_containers_deleted = result.containers_deleted.map(|d| d.len()).unwrap_or(0);
            let space_reclaimed_bytes = result.space_reclaimed;
            tracing::info!(
                num_containers_deleted,
                space_reclaimed_bytes,
                "Done pruning containers."
            );
        }
        Err(e) => tracing::error!(?e, "Error pruning containers."),
    }

    if prune_images {
        let filters: HashMap<String, Vec<String>> = vec![
            ("until".to_string(), vec![since_unixtime.to_string()]),
            // Ref: https://docs.docker.com/reference/api/engine/version/v1.50/#tag/Image/operation/ImagePrune
            // > When set to true (or 1), prune only unused and untagged images. When set to false (or 0), all unused images are pruned.
            // The documentation doesn't specify the default value, so we set it to false to ensure that all unused images are pruned.
            // Note that Docker does not allow images to be labeled after they are created, so we end up pruning all images whether
            // created by Plane or not.
            ("dangling".to_string(), vec!["false".to_string()]),
        ]
        .into_iter()
        .collect();
        match docker
            .prune_images(Some(PruneImagesOptions { filters }))
            .await
        {
            Ok(result) => {
                let num_images_deleted = result.images_deleted.map(|d| d.len()).unwrap_or(0);
                tracing::info!(num_images_deleted, "Pruning images.");
            }
            Err(e) => tracing::error!(?e, "Error pruning images."),
        }
    }

    Ok(())
}
