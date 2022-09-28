use crate::config::{DockerConfig, DockerConnection};
use anyhow::{anyhow, Result};
use bollard::{
    auth::DockerCredentials,
    container::{
        Config, CreateContainerOptions, LogOutput, LogsOptions, StartContainerOptions, Stats,
        StatsOptions, StopContainerOptions,
    },
    image::CreateImageOptions,
    models::{EventMessage, HostConfig, PortBinding, ResourcesUlimits},
    system::EventsOptions,
    Docker, API_DEFAULT_VERSION,
};
use dis_spawner::messages::agent::ResourceLimits;
use std::{collections::HashMap, net::IpAddr, time::Duration};
use tokio_stream::{wrappers::IntervalStream, Stream, StreamExt};

/// The port in the container which is exposed.
const CONTAINER_PORT: u16 = 8080;
const DEFAULT_DOCKER_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_DOCKER_STATS_INTERVAL_SECONDS: u64 = 10;

#[derive(Clone)]
pub struct DockerInterface {
    docker: Docker,
    runtime: Option<String>,
}

/// Helper trait for swallowing Docker not found errors.
trait AllowNotFound {
    /// Swallow a result if it is a success result or a NotFound; propagate it otherwise.
    fn allow_not_found(self) -> Result<(), bollard::errors::Error>;
}

impl<T> AllowNotFound for Result<T, bollard::errors::Error> {
    fn allow_not_found(self) -> Result<(), bollard::errors::Error> {
        match self {
            Ok(_) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The list of possible container events.
/// Comes from [Docker documentation](https://docs.docker.com/engine/reference/commandline/events/).
#[derive(Debug, PartialEq, Eq)]
pub enum ContainerEventType {
    Attach,
    Commit,
    Copy,
    Create,
    Destroy,
    Detach,
    Die,
    ExecCreate,
    ExecDetach,
    ExecDie,
    ExecStart,
    Export,
    HealthStatus,
    Kill,
    Oom,
    Pause,
    Rename,
    Resize,
    Restart,
    Start,
    Stop,
    Top,
    Unpause,
    Update,
}

#[allow(unused)]
#[derive(Debug)]
pub struct ContainerEvent {
    pub event: ContainerEventType,
    pub name: String,
}

impl ContainerEvent {
    pub fn from_event_message(event: &EventMessage) -> Option<Self> {
        let action = event.action.as_deref()?;
        let actor = event.actor.as_ref()?;
        let name: String = actor.attributes.as_ref()?.get("name")?.to_string();

        let event = match action {
            "attach" => ContainerEventType::Attach,
            "commit" => ContainerEventType::Commit,
            "copy" => ContainerEventType::Copy,
            "create" => ContainerEventType::Create,
            "destroy" => ContainerEventType::Destroy,
            "detach" => ContainerEventType::Detach,
            "die" => ContainerEventType::Die,
            "exec_create" => ContainerEventType::ExecCreate,
            "exec_detach" => ContainerEventType::ExecDetach,
            "exec_die" => ContainerEventType::ExecDie,
            "exec_start" => ContainerEventType::ExecStart,
            "export" => ContainerEventType::Export,
            "health_status" => ContainerEventType::HealthStatus,
            "kill" => ContainerEventType::Kill,
            "oom" => ContainerEventType::Oom,
            "pause" => ContainerEventType::Pause,
            "rename" => ContainerEventType::Rename,
            "resize" => ContainerEventType::Resize,
            "restart" => ContainerEventType::Restart,
            "start" => ContainerEventType::Start,
            "stop" => ContainerEventType::Stop,
            "top" => ContainerEventType::Top,
            "unpause" => ContainerEventType::Unpause,
            "update" => ContainerEventType::Update,
            _ => {
                tracing::info!(?action, "Unhandled container action.");
                return None;
            }
        };

        Some(ContainerEvent { event, name })
    }
}

trait MinuteExt {
    fn as_minutes(&self) -> u128;
}

impl MinuteExt for std::time::Duration {
    fn as_minutes(&self) -> u128 {
        (self.as_secs() / 60).into()
    }
}

fn make_exposed_ports(port: u16) -> Option<HashMap<String, HashMap<(), ()>>> {
    let dummy: HashMap<(), ()> = vec![].into_iter().collect();
    Some(vec![(format!("{}/tcp", port), dummy)].into_iter().collect())
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
        })
    }

    pub async fn container_events(&self) -> impl Stream<Item = ContainerEvent> {
        let options: EventsOptions<&str> = EventsOptions {
            since: None,
            until: None,
            filters: vec![("type", vec!["container"])].into_iter().collect(),
        };
        self.docker
            .events(Some(options))
            .filter_map(|event| match event {
                Ok(event) => ContainerEvent::from_event_message(&event),
                Err(error) => {
                    tracing::error!(?error, "Error tracking container terminations.");
                    None
                }
            })
    }

    pub fn get_logs(
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

    pub fn get_stats<'a>(
        &'a self,
        container_name: &'a str,
    ) -> impl Stream<Item = Result<Stats, bollard::errors::Error>> + 'a {
        let options = StatsOptions {
            stream: false,
            one_shot: true,
        };

        IntervalStream::new(
            // call stats once for every INTERVAL
            tokio::time::interval(Duration::from_secs(DEFAULT_DOCKER_STATS_INTERVAL_SECONDS)),
        )
        .then(move |_tick| async move {
            self.docker
                .stats(container_name, Some(options))
                .next()
                .await
                .expect("docker should always return a stat")
        })
    }

    #[allow(unused)]
    pub async fn pull_image(
        &self,
        image: &str,
        credentials: &Option<DockerCredentials>,
    ) -> Result<()> {
        let options = Some(CreateImageOptions {
            from_image: image,
            ..Default::default()
        });

        let mut result = self.docker.create_image(options, None, credentials.clone());
        while let Some(next) = result.next().await {
            next?;
        }

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

    pub async fn is_running(&self, container_name: &str) -> Result<(bool, Option<i64>)> {
        let container = match self.docker.inspect_container(container_name, None).await {
            Ok(container) => container,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => return Ok((false, None)),
            Err(err) => return Err(err.into()),
        };
        let state = container
            .state
            .ok_or_else(|| anyhow!("No state found for container."))?;

        let running = state
            .running
            .ok_or_else(|| anyhow!("State found but no running field for container."))?;

        let exit_code = if running { None } else { state.exit_code };

        Ok((running, exit_code))
    }

    pub async fn get_ip(&self, container_name: &str) -> Option<IpAddr> {
        let inspect = self
            .docker
            .inspect_container(container_name, None)
            .await
            .ok()?;

        let port = inspect.network_settings?.ip_address?;

        port.parse().ok()
    }

    /// Run the specified image and return the name of the created container.
    pub async fn run_container(
        &self,
        name: &str,
        image: &str,
        env: &HashMap<String, String>,
        resource_limits: &ResourceLimits,
    ) -> Result<()> {
        let env: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        // Build the container.
        let container_id = {
            let options: Option<CreateContainerOptions<String>> = Some(CreateContainerOptions {
                name: name.to_string(),
            });

            let config: Config<String> = Config {
                image: Some(image.to_string()),
                env: Some(env),
                exposed_ports: make_exposed_ports(CONTAINER_PORT),
                labels: Some(
                    vec![
                        ("dev.spawner.managed".to_string(), "true".to_string()),
                        ("dev.spawner.backend".to_string(), name.to_string()),
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
            result.id
        };

        // Start the container.
        {
            let options: Option<StartContainerOptions<&str>> = None;

            self.docker.start_container(&container_id, options).await?;
        };

        Ok(())
    }
}
