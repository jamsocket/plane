mod util;
use self::util::{
    get_ip_of_container, AllowNotFound, ContainerEvent, ContainerEventType, StatsStream,
};
use crate::{
    agent::{
        engine::{Engine, EngineBackendStatus},
        engines::docker::util::{make_exposed_ports, MinuteExt},
    },
    config::{DockerConfig, DockerConnection},
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bollard::{
    auth::DockerCredentials,
    container::{
        Config, CreateContainerOptions, LogOutput, LogsOptions, StartContainerOptions, Stats,
        StatsOptions, StopContainerOptions,
    },
    image::CreateImageOptions,
    models::{HostConfig, PortBinding, ResourcesUlimits},
    system::EventsOptions,
    Docker, API_DEFAULT_VERSION,
};
use plane_core::{
    messages::agent::ResourceLimits,
    messages::agent::{BackendStatsMessage, DroneLogMessage, SpawnRequest},
    timing::Timer,
    types::BackendId,
};
use std::{collections::HashMap, time::Duration};
use std::{net::SocketAddr, pin::Pin};
use tokio_stream::{wrappers::IntervalStream, Stream, StreamExt};

/// The port in the container which is exposed.
const CONTAINER_PORT: u16 = 8080;
const DEFAULT_DOCKER_TIMEOUT_SECONDS: u64 = 30;
/// Interval between reporting stats of a running backend.
/// NOTE: the minimum possible interval is 1 second.
const DEFAULT_DOCKER_STATS_INTERVAL_SECONDS: u64 = 10;

#[derive(Clone)]
pub struct DockerInterface {
    docker: Docker,
    runtime: Option<String>,
    network: Option<String>,
}

impl DockerInterface {
    pub async fn try_new(config: &DockerConfig) -> Result<Self> {
        let docker = match &config.connection {
            DockerConnection::Socket { socket } => Docker::connect_with_unix(
                socket,
                DEFAULT_DOCKER_TIMEOUT_SECONDS,
                API_DEFAULT_VERSION,
            )?,
            DockerConnection::Http { http } => Docker::connect_with_http(
                http,
                DEFAULT_DOCKER_TIMEOUT_SECONDS,
                API_DEFAULT_VERSION,
            )?,
        };

        Ok(DockerInterface {
            docker,
            runtime: config.runtime.clone(),
            network: config.network.clone(),
        })
    }

    fn get_logs(
        &self,
        container_name: &str,
    ) -> impl Stream<Item = Result<LogOutput, bollard::errors::Error>> {
        self.docker.logs(
            container_name,
            Some(LogsOptions {
                follow: true,
                stdout: true,
                stderr: true,
                since: 0,
                until: 0,
                timestamps: true,
                tail: "all",
            }),
        )
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
                tokio::time::interval(Duration::from_secs(DEFAULT_DOCKER_STATS_INTERVAL_SECONDS));
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

    /// Run the specified image and return the name of the created container.
    async fn run_container(
        &self,
        name: &str,
        image: &str,
        env: &HashMap<String, String>,
        resource_limits: &ResourceLimits,
    ) -> Result<()> {
        let env: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        // Build the container.
        let container_id = {
            let timer = Timer::new();
            let options: Option<CreateContainerOptions<String>> = Some(CreateContainerOptions {
                name: name.to_string(),
                platform: None,
            });

            let config: Config<String> = Config {
                image: Some(image.to_string()),
                env: Some(env),
                exposed_ports: make_exposed_ports(CONTAINER_PORT),
                labels: Some(
                    vec![
                        ("dev.plane.managed".to_string(), "true".to_string()),
                        ("dev.plane.backend".to_string(), name.to_string()),
                    ]
                    .into_iter()
                    .collect(),
                ),
                host_config: Some(HostConfig {
                    port_bindings: Some(
                        vec![(
                            format!("{}/tcp", CONTAINER_PORT),
                            Some(vec![PortBinding {
                                host_ip: None,
                                host_port: Some("0".to_string()),
                            }]),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    network_mode: self.network.clone(),
                    runtime: self.runtime.clone(),
                    cpu_period: resource_limits
                        .cpu_period
                        .map(|cpu_period| cpu_period.as_micros() as i64),
                    cpu_quota: resource_limits
                        .cpu_period_percent
                        .and_then(|cpu_period_percent| {
                            let cpu_period = resource_limits
                                .cpu_period
                                .unwrap_or(std::time::Duration::from_millis(100));
                            cpu_period
                                .saturating_mul(cpu_period_percent as u32)
                                .checked_div(100)
                                .map(|cpu_period_time| cpu_period_time.as_micros() as i64)
                        }),
                    ulimits: resource_limits.cpu_time_limit.map(|cpu_time_limit| {
                        vec![ResourcesUlimits {
                            name: Some("cpu".to_string()),
                            soft: Some(cpu_time_limit.as_minutes() as i64),
                            hard: Some(cpu_time_limit.as_minutes() as i64),
                        }]
                    }),
                    ..HostConfig::default()
                }),
                ..Config::default()
            };

            let result = self.docker.create_container(options, config).await?;
            tracing::info!(duration=?timer.duration(), %image, "Created container.");
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
        self.pull_image(
            &spawn_request.executable.image,
            &spawn_request
                .executable
                .credentials
                .as_ref()
                .map(|d| d.into()),
        )
        .await?;

        let backend_id = spawn_request.backend_id.to_resource_name();
        self.run_container(
            &backend_id,
            &spawn_request.executable.image,
            &spawn_request.executable.env,
            &spawn_request.executable.resource_limits,
        )
        .await?;
        tracing::info!(%backend_id, "Container is running.");

        Ok(())
    }

    async fn backend_status(&self, backend: &BackendId) -> Result<EngineBackendStatus> {
        let container_name = backend.to_resource_name();
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
            .ok_or_else(|| anyhow!("No state found for container."))?;

        let running = state
            .running
            .ok_or_else(|| anyhow!("State found but no running field for container."))?;

        if running {
            let ip = get_ip_of_container(&container)?;
            let addr = SocketAddr::new(ip, CONTAINER_PORT);

            Ok(EngineBackendStatus::Running { addr })
        } else {
            match state.exit_code {
                None => Ok(EngineBackendStatus::Terminated),
                Some(0) => Ok(EngineBackendStatus::Exited),
                Some(_) => Ok(EngineBackendStatus::Failed),
            }
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
    ) -> Pin<Box<dyn Stream<Item = BackendStatsMessage> + Send>> {
        let stream = Box::pin(self.get_stats(backend));
        let backend = backend.clone();

        Box::pin(StatsStream::new(backend, stream))
    }

    async fn stop(&self, backend: &BackendId) -> Result<()> {
        self.stop_container(&backend.to_resource_name()).await
    }
}
