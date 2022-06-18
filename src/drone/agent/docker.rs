use super::DockerOptions;
use anyhow::{anyhow, Result};
use bollard::{
    auth::DockerCredentials,
    container::{
        Config, CreateContainerOptions, LogsOptions, StartContainerOptions, StopContainerOptions,
    },
    image::CreateImageOptions,
    models::{EventMessage, HostConfig, PortBinding},
    network::CreateNetworkOptions,
    system::EventsOptions,
    Docker, API_DEFAULT_VERSION,
};
use std::collections::HashMap;
use tokio_stream::{Stream, StreamExt};

/// The port in the container which is exposed.
const CONTAINER_PORT: u16 = 8080;
const DEFAULT_DOCKER_TIMEOUT_SECONDS: u64 = 30;

#[derive(Clone)]
pub struct DockerInterface {
    docker: Docker,
    runtime: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum ContainerEventType {
    Start,
    Create,
    Die,
    Stop,
    Kill,
    Destroy,
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
            "die" => ContainerEventType::Die,
            "stop" => ContainerEventType::Stop,
            "start" => ContainerEventType::Start,
            "kill" => ContainerEventType::Kill,
            "create" => ContainerEventType::Create,
            "destroy" => ContainerEventType::Destroy,
            _ => {
                tracing::info!(?action, "Unhandled container action.");
                return None;
            }
        };

        Some(ContainerEvent { event, name })
    }
}

fn make_exposed_ports(port: u16) -> Option<HashMap<String, HashMap<(), ()>>> {
    let dummy: HashMap<(), ()> = vec![].into_iter().collect();
    Some(vec![(format!("{}/tcp", port), dummy)].into_iter().collect())
}

impl DockerInterface {
    pub async fn try_new(config: &DockerOptions) -> Result<Self> {
        let docker = match &config.transport {
            super::DockerApiTransport::Socket(docker_socket) => Docker::connect_with_unix(
                docker_socket,
                DEFAULT_DOCKER_TIMEOUT_SECONDS,
                API_DEFAULT_VERSION,
            )?,
            super::DockerApiTransport::Http(docker_http) => Docker::connect_with_http(
                docker_http,
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

        if let Ok(val) = self.is_running(name).await {
            if val.0 {
                self.docker.stop_container(name, Some(options)).await?;
            }
        }

        self.docker.remove_network(name).await?;

        Ok(())
    }

    pub async fn is_running(&self, container_name: &str) -> Result<(bool, Option<i64>)> {
        let container = self.docker.inspect_container(container_name, None).await?;
        let state = container
            .state
            .ok_or_else(|| anyhow!("No state found for container."))?;

        let running = state
            .running
            .ok_or_else(|| anyhow!("State found but no running field for container."))?;

        let exit_code = if running { None } else { state.exit_code };

        Ok((running, exit_code))
    }

    pub async fn get_port(&self, container_name: &str) -> Option<u16> {
        let inspect = self
            .docker
            .inspect_container(container_name, None)
            .await
            .ok()?;

        let port = inspect
            .network_settings
            .as_ref()?
            .ports
            .as_ref()?
            .get(&format!("{}/tcp", CONTAINER_PORT))?
            .as_ref()?
            .first()?
            .host_port
            .as_ref()?;

        port.parse().ok()
    }

    /// Run the specified image and return the name of the created container.
    pub async fn run_container(
        &self,
        name: &str,
        image: &str,
        env: &HashMap<String, String>,
    ) -> Result<String> {
        let env: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        // Build the network.
        let network_id = {
            let options: CreateNetworkOptions<&str> = CreateNetworkOptions {
                name,
                ..CreateNetworkOptions::default()
            };

            let response = self.docker.create_network(options).await?;
            response
                .id
                .ok_or_else(|| anyhow!("Network response contained no network id."))?
        };

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
                    network_mode: Some(network_id.clone()),
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

        Ok(container_id)
    }

    #[allow(dead_code)]
    pub async fn get_last_n_log_lines(&self, container_id: &str, n: usize) -> Result<Vec<String>> {
        let stream = self.docker.logs(
            &container_id,
            Some(LogsOptions {
                stdout: true,
                stderr: true,
                tail: n.to_string(),
                ..Default::default()
            }),
        );

        Ok(stream
            .map(|log| log.unwrap().to_string())
            .collect::<Vec<_>>()
            .await)
    }
}

#[cfg(feature = "docker-unit-test")]
#[cfg(test)]
mod test {
    use super::*;

    async fn get_docker_interface() -> DockerInterface {
        let docker_options = DockerOptions {
            transport: crate::drone::agent::DockerApiTransport::Socket(
                "/var/run/docker.sock".to_string(),
            ),
            runtime: None,
        };
        DockerInterface::try_new(&docker_options).await.unwrap()
    }

    #[tokio::test]
    async fn test_hello_world_img() {
        let docker = get_docker_interface().await;

        let container_name = "test";
        let image_name = "hello-world";

        let container_id = docker
            .run_container(&container_name, &image_name, &HashMap::new())
            .await
            .unwrap();
        let log_lines = docker.get_last_n_log_lines(&container_id, 2).await.unwrap();

        assert_eq!(log_lines.len(), 2);
        assert_eq!(log_lines[0], " https://docs.docker.com/get-started/\n");
        assert_eq!(log_lines[1], "\n");

        let stopped = docker.stop_container(&container_name).await;

        assert_eq!(stopped.is_ok(), true);

        let removed = docker.docker.remove_container(&container_id, None).await;
        assert_eq!(removed.is_ok(), true);
    }
}
