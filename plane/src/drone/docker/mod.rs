use self::{
    commands::{get_port, run_container},
    types::ContainerId,
};
use crate::{
    database::backend::BackendMetricsMessage,
    heartbeat_consts::KILL_AFTER_SOFT_TERMINATE_SECONDS,
    names::BackendName,
    protocol::AcquiredKey,
    types::{BearerToken, DockerExecutorConfig, PullPolicy},
    util::GuardHandle,
};
use anyhow::Result;
use bollard::{
    container::{PruneContainersOptions, StatsOptions, StopContainerOptions},
    errors::Error,
    image::PruneImagesOptions,
    service::{EventMessage, HostConfigLogConfig},
    system::EventsOptions,
    Docker,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::{collections::HashMap, path::PathBuf};
use thiserror::Error;
use tokio_stream::{Stream, StreamExt};
use valuable::Valuable;

/// Clean up containers and images every minute.
const CLEANUP_INTERVAL_SECS: i64 = 60;

pub mod commands;
pub mod types;

/// The label used to identify containers managed by Plane.
/// The existence of this label is used to determine whether a container is managed by Plane.
const PLANE_DOCKER_LABEL: &str = "dev.plane.backend";

fn backend_id_to_container_id(backend_id: &BackendName) -> ContainerId {
    ContainerId::from(format!("plane-{}", backend_id.to_string()))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DockerRuntimeConfig {
    pub runtime: Option<String>,
    pub log_config: Option<HostConfigLogConfig>,
    pub mount_base: Option<PathBuf>,

    pub auto_prune: Option<bool>,
    #[serde(with = "crate::serialization::serialize_optional_duration_as_seconds")]
    pub cleanup_min_age: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct DockerRuntime {
    docker: Docker,
    config: DockerRuntimeConfig,
    _cleanup_handle: Arc<GuardHandle>,
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

impl DockerRuntime {
    pub async fn new(config: DockerRuntimeConfig) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()?;

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

        let cleanup_handle = Arc::new(cleanup_handle);

        Ok(Self {
            docker,
            config,
            _cleanup_handle: cleanup_handle,
        })
    }

    pub async fn prepare(&self, config: &DockerExecutorConfig) -> Result<()> {
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

    pub async fn events(&self) -> impl Stream<Item = TerminateEvent> {
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

            tracing::info!(event=?e, "Received event");

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
                    tracing::warn!(
                        ?err,
                        backend_id = backend_id.as_value(),
                        "Ignoring event with invalid backend ID."
                    );
                    return None;
                }
            };

            tracing::info!(
                exit_code,
                backend_id = backend_id.as_value(),
                "Received exit code"
            );

            Some(TerminateEvent {
                backend_id,
                exit_code,
            })
        })
    }

    pub async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: DockerExecutorConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult> {
        let container_id =
            run_container(self, backend_id, executable, acquired_key, static_token).await?;
        let port = get_port(&self.docker, &container_id).await?;

        Ok(SpawnResult {
            container_id: container_id.clone(),
            port,
        })
    }

    pub async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<(), Error> {
        let container_id = backend_id_to_container_id(backend_id);

        if hard {
            self.docker
                .kill_container::<String>(&container_id.to_string(), None)
                .await?;
        } else {
            self.docker
                .stop_container(
                    &container_id.to_string(),
                    Some(StopContainerOptions {
                        t: KILL_AFTER_SOFT_TERMINATE_SECONDS,
                    }),
                )
                .await?;
        }

        Ok(())
    }

    pub async fn metrics(&self, backend_id: &BackendName) -> Result<bollard::container::Stats> {
        let container_id = backend_id_to_container_id(backend_id);

        let options = StatsOptions {
            stream: false,
            one_shot: false,
        };

        self.docker
            .stats(&container_id.to_string(), Some(options))
            .next()
            .await
            .ok_or(anyhow::anyhow!("no stats found for {container_id}"))?
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }
}

#[derive(Error, Debug)]
pub enum MetricsConversionError {
    #[error("{0} stat not present in provided struct")]
    NoStatsAvailable(String),
    #[error("Measured cumulative system cpu use: {current} is less than previous cumulative total: {prev}")]
    SysCpuLessThanCurrent { current: u64, prev: u64 },
    #[error("Measured cumulative container cpu use: {current} is less than previous cumulative total: {prev}")]
    ContainerCpuLessThanCurrent { current: u64, prev: u64 },
}

pub fn get_metrics_message_from_container_stats(
    stats: bollard::container::Stats,
    backend_id: BackendName,
    prev_sys_cpu_ns: &AtomicU64,
    prev_container_cpu_ns: &AtomicU64,
) -> std::result::Result<BackendMetricsMessage, MetricsConversionError> {
    let Some(mem_stats) = stats.memory_stats.stats else {
        return Err(MetricsConversionError::NoStatsAvailable(
            "memory_stats.stats".into(),
        ));
    };

    let Some(total_system_cpu_used) = stats.cpu_stats.system_cpu_usage else {
        return Err(MetricsConversionError::NoStatsAvailable(
            "cpu_stats.system_cpu_usage".into(),
        ));
    };

    let Some(mem_used_total_docker) = stats.memory_stats.usage else {
        return Err(MetricsConversionError::NoStatsAvailable(
            "memory_stats.usage".into(),
        ));
    };

    let Some(mem_limit) = stats.memory_stats.limit else {
        return Err(MetricsConversionError::NoStatsAvailable(
            "memory_stats.limit".into(),
        ));
    };

    let container_cpu_used = stats.cpu_stats.cpu_usage.total_usage;
    let prev_sys_cpu_ns_load = prev_sys_cpu_ns.load(Ordering::SeqCst);
    let prev_container_cpu_ns_load = prev_container_cpu_ns.load(Ordering::SeqCst);

    if container_cpu_used < prev_container_cpu_ns_load {
        return Err(MetricsConversionError::ContainerCpuLessThanCurrent {
            current: container_cpu_used,
            prev: prev_container_cpu_ns_load,
        });
    }
    if total_system_cpu_used < prev_sys_cpu_ns_load {
        return Err(MetricsConversionError::SysCpuLessThanCurrent {
            current: total_system_cpu_used,
            prev: prev_sys_cpu_ns_load,
        });
    }

    let container_cpu_used_delta =
        container_cpu_used - prev_container_cpu_ns.load(Ordering::SeqCst);

    let system_cpu_used_delta = total_system_cpu_used - prev_sys_cpu_ns_load;

    prev_container_cpu_ns.store(container_cpu_used, Ordering::SeqCst);
    prev_sys_cpu_ns.store(total_system_cpu_used, Ordering::SeqCst);

    // NOTE: a BIG limitation here is that docker does not report swap info!
    // This may be important for scheduling decisions!

    let (mem_total, mem_active, mem_inactive, mem_unevictable, mem_used) = match mem_stats {
        bollard::container::MemoryStatsStats::V1(v1_stats) => {
            let active_mem = v1_stats.total_active_anon + v1_stats.total_active_file;
            let total_mem = v1_stats.total_rss + v1_stats.total_cache;
            let unevictable_mem = v1_stats.total_unevictable;
            let inactive_mem = v1_stats.total_inactive_anon + v1_stats.total_inactive_file;
            let mem_used = mem_used_total_docker - v1_stats.total_inactive_file;
            (
                total_mem,
                active_mem,
                inactive_mem,
                unevictable_mem,
                mem_used,
            )
        }
        bollard::container::MemoryStatsStats::V2(v2_stats) => {
            let active_mem = v2_stats.active_anon + v2_stats.active_file;
            let kernel = v2_stats.kernel_stack + v2_stats.sock + v2_stats.slab;
            let total_mem = v2_stats.file + v2_stats.anon + kernel;
            let unevictable_mem = v2_stats.unevictable;
            let inactive_mem = v2_stats.inactive_anon + v2_stats.inactive_file;
            let mem_used = mem_used_total_docker - v2_stats.inactive_file;
            (
                total_mem,
                active_mem,
                inactive_mem,
                unevictable_mem,
                mem_used,
            )
        }
    };

    Ok(BackendMetricsMessage {
        backend_id,
        mem_total,
        mem_used,
        mem_active,
        mem_inactive,
        mem_unevictable,
        mem_limit,
        cpu_used: container_cpu_used_delta,
        sys_cpu: system_cpu_used_delta,
    })
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
        let filters: HashMap<String, Vec<String>> =
            vec![("until".to_string(), vec![since_unixtime.to_string()])]
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
