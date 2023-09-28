mod util;
use self::util::{
    get_ip_of_container, AllowNotFound, ContainerEvent, ContainerEventType, StatsStream,
};
use crate::{
    agent::{
        engine::{Engine, EngineBackendStatus},
        engines::docker::util::MinuteExt,
    },
    config::{DockerConfig, DockerConnection},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bollard::{
    auth::DockerCredentials,
    container::{
        Config, CreateContainerOptions, LogOutput, LogsOptions, StartContainerOptions, Stats,
        StatsOptions, StopContainerOptions,
    },
    image::CreateImageOptions,
    models::{HostConfig, ResourcesUlimits},
    service::{DeviceRequest, HostConfigLogConfig, Mount},
    system::EventsOptions,
    Docker, API_DEFAULT_VERSION,
};
use plane_core::{
    messages::agent::{
        BackendStatsMessage, DockerExecutableConfig, DockerPullPolicy, DroneLogMessage,
        SpawnRequest,
    },
    timing::Timer,
    types::{BackendId, ClusterName, DroneId},
};
use std::time::Duration;
use std::{net::SocketAddr, pin::Pin};
use tokio_stream::{wrappers::IntervalStream, Stream, StreamExt};

/// The port in the container which is exposed.
const DEFAULT_CONTAINER_PORT: u16 = 8080;
const DOCKER_TIMEOUT_SECONDS: u64 = 30;
/// Interval between reporting stats of a running backend.
/// NOTE: the minimum possible interval is 1 second.
const DOCKER_STATS_INTERVAL_SECONDS: u64 = 5;

#[derive(Clone)]
pub struct DockerInterface {
    docker: Docker,
    runtime: Option<String>,
    network: Option<String>,
    gpu: bool,
    allow_volume_mounts: bool,
    syslog: Option<String>,
}

impl DockerInterface {
    pub async fn try_new(config: &DockerConfig) -> Result<Self> {
        let docker = match &config.connection {
            DockerConnection::Socket { socket } => {
                Docker::connect_with_unix(socket, DOCKER_TIMEOUT_SECONDS, API_DEFAULT_VERSION)?
            }
            DockerConnection::Http { http } => {
                Docker::connect_with_http(http, DOCKER_TIMEOUT_SECONDS, API_DEFAULT_VERSION)?
            }
        };

        Ok(DockerInterface {
            docker,
            runtime: config.runtime.clone(),
            network: config.network.clone(),
            gpu: config.insecure_gpu,
            allow_volume_mounts: config.allow_volume_mounts,
            syslog: config.syslog.clone(),
        })
    }

    fn get_logs(
        &self,
        container_name: &str,
    ) -> impl Stream<Item = Result<LogOutput, bollard::errors::Error>> {
        let opts = LogsOptions {
            follow: true,
            stdout: true,
            stderr: true,
            since: 0,
            until: 0,
            timestamps: true,
            tail: "all",
        };

        self.docker.logs(container_name, Some(opts.clone()))
    }

    /// The docker api (as of docker version 20.10.18) blocks for ~1s before returning
    /// from self.docker.stats, hence the effective minimal interval is a second
    fn get_stats(&self, backend_id: &BackendId) -> impl Stream<Item = Stats> {
        let options = StatsOptions {
            stream: false,
            one_shot: true,
        };

        let ticker = IntervalStream::new({
            let mut ticker =
                tokio::time::interval(Duration::from_secs(DOCKER_STATS_INTERVAL_SECONDS));
            // this prevents the stream from getting too big in case the ticker interval <1s
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker
        });

        let backend_id = backend_id.clone();
        let resource_name = backend_id.to_resource_name();
        let docker = self.docker.clone();

        futures::stream::StreamExt::filter_map(ticker, move |_tick| {
            let resource_name = resource_name.clone();
            let docker = docker.clone();
            async move {
                docker
                    .stats(&resource_name, Some(options))
                    .next()
                    .await
                    .and_then(|d| d.ok())
            }
        })
    }

    async fn pull_image(&self, image: &str, credentials: &Option<DockerCredentials>) -> Result<()> {
        let timer = Timer::new();
        let options = Some(CreateImageOptions {
            from_image: image,
            ..Default::default()
        });

        let mut result = self.docker.create_image(options, None, credentials.clone());
        while let Some(next) = result.next().await {
            next?;
        }

        tracing::info!(duration=?timer.duration(), ?image, "Pulled image.");

        Ok(())
    }

    pub async fn stop_container(&self, name: &str) -> Result<()> {
        let options = StopContainerOptions { t: 10 };

        self.docker
            .stop_container(name, Some(options))
            .await
            .allow_not_found()?;
        self.docker
            .remove_container(name, None)
            .await
            .allow_not_found()?;

        Ok(())
    }

    /// returns true if image exists, returns false in all other cases
    pub async fn image_exists(&self, image: &str) -> Result<bool> {
        match self.docker.inspect_image(image).await {
            Ok(..) => Ok(true),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Run the specified image and return the name of the created container.
    async fn run_container(
        &self,
        name: &str,
        executable_config: &DockerExecutableConfig,
    ) -> Result<()> {
        let env: Vec<String> = executable_config
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let device_requests = if self.gpu {
            Some(vec![DeviceRequest {
                count: Some(-1),
                capabilities: Some(vec![vec!["gpu".to_string()]]),
                ..Default::default()
            }])
        } else {
            None
        };

        let mounts = if self.allow_volume_mounts {
            let c: std::result::Result<Vec<Mount>, serde_json::Error> = executable_config
                .volume_mounts
                .iter()
                .map(|value| serde_json::from_value(value.clone()))
                .collect();

            Some(c?)
        } else {
            if !executable_config.volume_mounts.is_empty() {
                return Err(anyhow::anyhow!("Spawn attempt named volume mounts, but these are not enabled on this drone. Set `allow_volume_mounts` to true in the `docker` section of the Plane config to enable."));
            }

            None
        };

        // Create the container.
        let container_id = {
            let timer = Timer::new();
            let options: Option<CreateContainerOptions<String>> = Some(CreateContainerOptions {
                name: name.to_string(),
                platform: None,
            });

            let config: Config<String> = Config {
                image: Some(executable_config.image.to_string()),
                env: Some(env),
                labels: Some(
                    vec![
                        ("dev.plane.managed".to_string(), "true".to_string()),
                        ("dev.plane.backend".to_string(), name.to_string()),
                    ]
                    .into_iter()
                    .collect(),
                ),
                host_config: Some(HostConfig {
                    network_mode: self.network.clone(),
                    runtime: self.runtime.clone(),
                    memory: executable_config.resource_limits.memory_limit_bytes,
                    cpu_period: executable_config
                        .resource_limits
                        .cpu_period
                        .map(|cpu_period| cpu_period.as_micros() as i64),
                    cpu_quota: executable_config
                        .resource_limits
                        .cpu_period_percent
                        .and_then(|cpu_period_percent| {
                            let cpu_period = executable_config
                                .resource_limits
                                .cpu_period
                                .unwrap_or(std::time::Duration::from_millis(100));
                            cpu_period
                                .saturating_mul(cpu_period_percent as u32)
                                .checked_div(100)
                                .map(|cpu_period_time| cpu_period_time.as_micros() as i64)
                        }),
                    ulimits: executable_config.resource_limits.cpu_time_limit.map(
                        |cpu_time_limit| {
                            vec![ResourcesUlimits {
                                name: Some("cpu".to_string()),
                                soft: Some(cpu_time_limit.as_minutes() as i64),
                                hard: Some(cpu_time_limit.as_minutes() as i64),
                            }]
                        },
                    ),
                    storage_opt: executable_config
                        .resource_limits
                        .disk_limit_bytes
                        .map(|lim| {
                            vec![("size".to_string(), lim.to_string())]
                                .into_iter()
                                .collect()
                        }),
                    device_requests,
                    mounts,

                    log_config: self.syslog.as_ref().map(|d| HostConfigLogConfig {
                        typ: Some("syslog".to_string()),
                        config: Some(
                            vec![
                                ("syslog-address".to_string(), d.to_string()),
                                ("syslog-format".to_string(), "rfc5424".to_string()),
                                ("tag".to_string(), name.to_string()),
                            ]
                            .into_iter()
                            .collect(),
                        ),
                    }),

                    ..HostConfig::default()
                }),
                ..Config::default()
            };

            let result = self.docker.create_container(options, config).await?;
            tracing::info!(duration=?timer.duration(), image=%executable_config.image, "Created container.");
            result.id
        };

        // Start the container.
        {
            let timer = Timer::new();
            let options: Option<StartContainerOptions<&str>> = None;

            self.docker.start_container(&container_id, options).await?;
            tracing::info!(duation=?timer.duration(), %container_id, "Started container.");
        };

        Ok(())
    }
}

#[async_trait]
impl Engine for DockerInterface {
    fn interrupt_stream(&self) -> Pin<Box<dyn Stream<Item = plane_core::types::BackendId> + Send>> {
        let options: EventsOptions<&str> = EventsOptions {
            since: None,
            until: None,
            filters: vec![("type", vec!["container"])].into_iter().collect(),
        };
        let stream = self
            .docker
            .events(Some(options))
            .filter_map(|event| match event {
                Ok(event) => {
                    let event = ContainerEvent::from_event_message(&event)?;
                    if event.event == ContainerEventType::Die {
                        BackendId::from_resource_name(&event.name)
                    } else {
                        None
                    }
                }
                Err(error) => {
                    tracing::error!(?error, "Error tracking container terminations.");
                    None
                }
            });

        Box::pin(stream)
    }

    async fn load(&self, spawn_request: &SpawnRequest) -> Result<()> {
        let should_pull_image = match spawn_request.executable.pull_policy {
            DockerPullPolicy::Always => true,
            DockerPullPolicy::IfNotPresent => {
                !self.image_exists(&spawn_request.executable.image).await?
            }
            DockerPullPolicy::Never => false,
        };
        tracing::info!(?should_pull_image, pull_policy=?spawn_request.executable.pull_policy, "Image pull decision made.");

        if should_pull_image {
            self.pull_image(
                &spawn_request.executable.image,
                &spawn_request
                    .executable
                    .credentials
                    .as_ref()
                    .map(|d| d.into()),
            )
            .await?;
        }

        let backend_id = spawn_request.backend_id.to_resource_name();
        self.run_container(&backend_id, &spawn_request.executable)
            .await?;
        tracing::info!(%backend_id, "Container is running.");

        Ok(())
    }

    async fn backend_status(&self, spawn_request: &SpawnRequest) -> Result<EngineBackendStatus> {
        let container_name = spawn_request.backend_id.to_resource_name();
        let container = match self.docker.inspect_container(&container_name, None).await {
            Ok(container) => container,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => return Ok(EngineBackendStatus::Unknown),
            Err(err) => return Err(err.into()),
        };
        let state = container
            .state
            .as_ref()
            .context("No state found for container.")?;

        let running = state
            .running
            .context("State found but no running field for container.")?;

        if running {
            let ip = get_ip_of_container(&container)?;
            let addr = SocketAddr::new(
                ip,
                spawn_request
                    .executable
                    .port
                    .unwrap_or(DEFAULT_CONTAINER_PORT),
            );

            Ok(EngineBackendStatus::Running { addr })
        } else {
            let status = match state.exit_code {
                None => EngineBackendStatus::Terminated,
                Some(0) => EngineBackendStatus::Exited,
                Some(code) => EngineBackendStatus::Failed {
                    code: code.try_into().unwrap(),
                },
            };

            let state_json = serde_json::to_string(&state)
                .unwrap_or_else(|_| "Failed to serialize container state.".into());

            tracing::info!(?status, state=%state_json, "Set to terminal state.");

            Ok(status)
        }
    }

    fn log_stream(
        &self,
        backend: &BackendId,
    ) -> Pin<Box<dyn Stream<Item = DroneLogMessage> + Send>> {
        let stream = self.get_logs(&backend.to_resource_name());
        let backend = backend.clone();
        let stream = stream.filter_map(move |v| {
            v.ok()
                .as_ref()
                .and_then(|d| DroneLogMessage::from_log_message(&backend, d))
        });
        Box::pin(stream)
    }

    fn stats_stream(
        &self,
        backend: &BackendId,
        drone: &DroneId,
        cluster: &ClusterName,
    ) -> Pin<Box<dyn Stream<Item = BackendStatsMessage> + Send>> {
        let stream = Box::pin(self.get_stats(backend));
        let backend = backend.clone();
        let drone = drone.clone();

        Box::pin(StatsStream::new(backend, drone, cluster.clone(), stream))
    }

    async fn stop(&self, backend: &BackendId) -> Result<()> {
        self.stop_container(&backend.to_resource_name()).await
    }
}
