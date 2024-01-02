use anyhow::Result;
use bollard::{
    container::{Config, CreateContainerOptions, LogOutput},
    Docker,
};
use futures_util::StreamExt;
use std::{path::PathBuf, time::Duration};
use tokio::time::sleep;

pub struct Container {
    docker: Docker,
    pub container_id: String,
    pub name: String,
    pub log_to: Option<PathBuf>,
}

impl Container {
    pub async fn create(
        name: String,
        docker: Docker,
        config: Config<String>,
        log_to: Option<PathBuf>,
    ) -> Result<Self> {
        let image = config
            .image
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No image specified."))?;

        if !image_exists(&docker, image).await? {
            pull_image(&docker, image).await?;
        }

        let create_container_options = CreateContainerOptions {
            name: name.clone(),
            ..Default::default()
        };

        let container_response = docker
            .create_container(Some(create_container_options), config)
            .await?;

        let container_id = container_response.id;

        docker.start_container::<&str>(&container_id, None).await?;

        Ok(Self {
            docker,
            container_id,
            name,
            log_to,
        })
    }

    async fn try_get_port(&self, container_port: u16) -> Result<u16> {
        let container_info = self
            .docker
            .inspect_container(&self.container_id, None)
            .await?;

        let network_settings = container_info
            .network_settings
            .ok_or_else(|| anyhow::anyhow!("Container had no network settings."))?;

        let port = network_settings
            .ports
            .ok_or_else(|| anyhow::anyhow!("Container had no ports."))?;

        let port = port
            .get(&format!("{}/tcp", container_port))
            .ok_or_else(|| anyhow::anyhow!("Container had no port {}/tcp.", container_port))?
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Container had no port {}/tcp.", container_port))?
            .first()
            .ok_or_else(|| anyhow::anyhow!("Container had no port {}/tcp.", container_port))?;

        let port = port
            .host_port
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Container had no port {}/tcp.", container_port))?;

        Ok(port.parse()?)
    }

    pub async fn get_port(&self, container_port: u16) -> Result<u16> {
        // There can be a race condition where the container is ready but has
        // not yet received a port assignment, so we retry a few times.
        for _ in 0..3 {
            match self.try_get_port(container_port).await {
                Ok(port) => return Ok(port),
                Err(e) => {
                    tracing::info!(?e, "Failed to get port, retrying...");
                }
            }
            sleep(Duration::from_millis(100)).await;
        }

        Err(anyhow::anyhow!("Failed to get port after retries."))
    }

    pub async fn collect_stdout(&self) -> Result<Vec<u8>> {
        let mut stdout_buffer = Vec::new();

        let mut stream = self.docker.logs::<String>(
            &self.container_id,
            Some(bollard::container::LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );

        while let Some(next) = stream.next().await {
            let chunk = next?;
            match chunk {
                LogOutput::StdOut { message } => {
                    stdout_buffer.extend(message);
                }
                _ => {}
            }
        }

        Ok(stdout_buffer)
    }

    pub async fn check_exit_code(&self) -> Result<()> {
        let container_info = self
            .docker
            .inspect_container(&self.container_id, None)
            .await?;
        if container_info.state.as_ref().map(|s| s.exit_code) != Some(Some(0)) {
            return Err(anyhow::anyhow!("Container exited with non-zero exit code."));
        };

        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.docker.stop_container(&self.container_id, None).await?;

        if let Some(log_to) = &self.log_to {
            let log_stream = self.collect_stdout().await?;
            std::fs::write(log_to.join(format!("{}.txt", self.name)), log_stream)?;
        }

        self.docker
            .remove_container(&self.container_id, None)
            .await?;

        Ok(())
    }
}

pub async fn image_exists(docker: &Docker, image: &str) -> Result<bool> {
    match docker.inspect_image(image).await {
        Ok(..) => Ok(true),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub async fn pull_image(docker: &Docker, image: &str) -> Result<()> {
    let options = bollard::image::CreateImageOptions {
        from_image: image,
        ..Default::default()
    };

    let mut result = docker.create_image(Some(options), None, None);
    // create_image returns a stream; the image is not fully pulled until the stream is consumed.
    while let Some(next) = result.next().await {
        let _ = next?;
    }

    tracing::info!(?image, "Pulled image.");

    Ok(())
}
