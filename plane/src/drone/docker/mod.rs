use self::{
    commands::{get_port, run_container},
    types::ContainerId,
};
use crate::{
    database::backend::BackendMetricsMessage,
    names::BackendName,
    protocol::AcquiredKey,
    types::{BearerToken, ExecutorConfig},
};
use anyhow::Result;
use bollard::{
    auth::DockerCredentials,
    container::StatsOptions,
    errors::Error,
    service::{EventMessage, HostConfigLogConfig},
    system::EventsOptions,
    Docker,
};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio_stream::{Stream, StreamExt};

mod commands;
pub mod types;

/// The label used to identify containers managed by Plane.
/// The existence of this label is used to determine whether a container is managed by Plane.
const PLANE_DOCKER_LABEL: &str = "dev.plane.backend";

#[derive(Clone, Debug)]
pub struct PlaneDocker {
    docker: Docker,
    runtime: Option<String>,
    log_config: Option<HostConfigLogConfig>,
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
    pub async fn new(
        docker: Docker,
        runtime: Option<String>,
        log_config: Option<HostConfigLogConfig>,
    ) -> Result<Self> {
        Ok(Self {
            docker,
            runtime,
            log_config,
        })
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
                    tracing::warn!(?err, backend_id, "Ignoring event with invalid backend ID.");
                    return None;
                }
            };

            tracing::info!(exit_code, "Received exit code");

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
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult> {
        run_container(
            &self.docker,
            backend_id,
            container_id,
            executable,
            self.runtime.as_deref(),
            acquired_key,
            static_token,
            self.log_config.as_ref(),
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

    pub async fn get_metrics(
        &self,
        container_id: &ContainerId,
    ) -> Result<bollard::container::Stats> {
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
