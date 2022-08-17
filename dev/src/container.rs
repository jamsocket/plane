use anyhow::Result;
use bollard::{
    container::{Config, StartContainerOptions},
    image::CreateImageOptions,
    Docker,
};
use std::collections::HashMap;
use tokio_stream::StreamExt;

use crate::TEARDOWN_TASK_MANAGER;

#[derive(Clone)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub environment: HashMap<String, String>,
    pub command: Vec<String>,
    pub volumes: Vec<String>,
}

pub struct ContainerResource {
    docker: Docker,
    container_id: String,
}

impl ContainerResource {
    pub async fn new(spec: &ContainerSpec) -> Result<ContainerResource> {
        let docker = Docker::connect_with_unix_defaults()?;

        tracing::info!(image_name=%spec.image, "Pulling container.");

        pull_image(&docker, &spec.image).await?;

        tracing::info!(image_name=%spec.image, "Starting container.");

        let env: Vec<String> = spec
            .environment
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // This is ugly but it's really the API.
        let volumes: HashMap<String, HashMap<(), ()>> = spec
            .volumes
            .iter()
            .map(|v| (v.clone(), HashMap::new()))
            .collect();

        let result = docker
            .create_container::<String, String>(
                None,
                Config {
                    hostname: Some(spec.name.to_string()),
                    env: Some(env),
                    cmd: Some(spec.command.clone()),
                    image: Some(spec.image.to_string()),
                    volumes: Some(volumes),
                    ..Config::default()
                },
            )
            .await?;

        {
            let options: Option<StartContainerOptions<&str>> = None;
            docker.start_container(&result.id, options).await?;
        };

        tracing::info!(%result.id, "Container started.");

        Ok(ContainerResource {
            docker,
            container_id: result.id.clone(),
        })
    }
}

impl Drop for ContainerResource {
    fn drop(&mut self) {
        let docker = self.docker.clone();
        let container_id = self.container_id.clone();

        TEARDOWN_TASK_MANAGER.with(|manager| {
            manager.add_task(async move {
                tracing::info!(?container_id, "Stopping container");
                docker.stop_container(&container_id, None).await.expect("Error stopping container.");
                tracing::info!(?container_id, "Removing container");
                docker.remove_container(&container_id, None).await.expect("Error removing container.");
                Ok(())
            })
        });

    }
}

/// White out a line and return to the beginning. The number of spaces is not scientific;
const CLEAR_LINE: &str =
    "                                                                                        \r";

pub async fn pull_image(docker: &Docker, name: &str) -> Result<()> {
    let mut stream = docker.create_image(
        Some(CreateImageOptions {
            from_image: name.to_string(),
            ..CreateImageOptions::default()
        }),
        None,
        None,
    );

    while let Some(pull_event) = stream.next().await {
        if let Some(event_message) = pull_event?.status {
            let event_message = event_message.trim();
            print!("{}", CLEAR_LINE);
            print!("{}", event_message);
        }
    }
    println!();

    Ok(())
}
