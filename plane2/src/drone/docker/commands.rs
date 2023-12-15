use super::types::ContainerId;
use crate::{names::BackendName, types::ExecutorConfig};
use anyhow::Result;
use bollard::{
    service::{HostConfig, PortBinding, ResourcesUlimits},
    Docker,
};
use futures_util::StreamExt;
use std::{collections::HashMap, time::Duration};

/// Port inside the container to expose.
const CONTAINER_PORT: u16 = 8080;

pub async fn pull_image(docker: &Docker, image: &str, force: bool) -> Result<()> {
    if !force && image_exists(docker, image).await? {
        tracing::info!(?image, "Skipping image that already exists.");
        return Ok(());
    }

    let options = bollard::image::CreateImageOptions {
        from_image: image,
        ..Default::default()
    };

    let mut result = docker.create_image(Some(options), None, None);
    // create_image returns a stream; the image is not fully pulled until the stream is consumed.
    while let Some(next) = result.next().await {
        let info = next?;
        if let Some(progress) = info.progress_detail {
            tracing::info!(?progress, "Image pull progress.");
        }
    }

    tracing::info!(?image, "Pulled image.");

    Ok(())
}

fn create_labels(backend_id: &BackendName) -> HashMap<String, String> {
    vec![(
        super::PLANE_DOCKER_LABEL.to_string(),
        backend_id.to_string(),
    )]
    .into_iter()
    .collect()
}

fn create_port_bindings() -> HashMap<String, Option<Vec<PortBinding>>> {
    vec![(
        format!("{}/tcp", CONTAINER_PORT),
        Some(vec![bollard::models::PortBinding {
            host_ip: None,
            host_port: None,
        }]),
    )]
    .into_iter()
    .collect()
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

async fn try_get_port(docker: &Docker, container_id: &ContainerId) -> Result<u16> {
    let info = docker
        .inspect_container(&container_id.to_string(), None)
        .await?;

    // TODO: avoid clones.

    let port = info
        .network_settings
        .and_then(|settings| settings.ports)
        .and_then(|ports| ports.get(&format!("{}/tcp", CONTAINER_PORT)).cloned())
        .and_then(|bindings| bindings.clone())
        .and_then(|bindings| bindings.get(0).cloned())
        .and_then(|binding| binding.host_port.clone())
        .and_then(|port| port.parse::<u16>().ok())
        .ok_or_else(|| anyhow::anyhow!("Failed to get port for container."))?;

    Ok(port)
}

pub async fn get_port(docker: &Docker, container_id: &ContainerId) -> Result<u16> {
    // There can be a race condition where the container is ready but has
    // not yet received a port assignment, so we retry a few times.
    for _ in 0..3 {
        match try_get_port(docker, container_id).await {
            Ok(port) => return Ok(port),
            Err(e) => {
                tracing::info!(?e, "Failed to get port, retrying...");
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(anyhow::anyhow!("Failed to get port after retries."))
}

pub async fn run_container(
    docker: &Docker,
    backend_id: &BackendName,
    container_id: &ContainerId,
    exec_config: ExecutorConfig,
    runtime: &Option<String>,
) -> Result<()> {
    let options = bollard::container::CreateContainerOptions {
        name: container_id.to_string(),
        ..Default::default()
    };

    let mut env = exec_config.env;
    env.insert("PORT".to_string(), CONTAINER_PORT.to_string());
    env.insert("PLANE_BACKEND_ID".to_string(), backend_id.to_string());
    // TODO: set PLANE_LOCK and PLANE_FENCING_TOKEN.
    let env: Vec<String> = env
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    let config = bollard::container::Config {
        image: Some(exec_config.image.clone()),
        labels: Some(create_labels(backend_id)),
        env: Some(env),
        exposed_ports: Some(
            vec![(format!("{}/tcp", CONTAINER_PORT), HashMap::new())]
                .into_iter()
                .collect(),
        ),
        host_config: Some(HostConfig {
            port_bindings: Some(create_port_bindings()),
            runtime: runtime.clone(),
            memory: exec_config.resource_limits.memory_limit_bytes,
            cpu_period: exec_config
                .resource_limits
                .cpu_period
                .as_ref()
                .map(|cpu_period| std::time::Duration::from(cpu_period).as_micros() as i64),
            cpu_quota: exec_config
                .resource_limits
                .cpu_quota()
                .map(|cpu_quota| cpu_quota.as_micros() as i64),
            ulimits: exec_config
                .resource_limits
                .cpu_time_limit
                .map(|cpu_time_limit| {
                    let mins = (cpu_time_limit.as_secs_f64() / 60.0).floor() as i64;
                    vec![ResourcesUlimits {
                        name: Some("cpu".to_string()),
                        soft: Some(mins),
                        hard: Some(mins),
                    }]
                }),
            storage_opt: exec_config.resource_limits.disk_limit_bytes.map(|lim| {
                let mut hm = HashMap::new();
                hm.insert("size".to_string(), lim.to_string());
                hm
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    docker.create_container(Some(options), config).await?;

    docker
        .start_container::<String>(&container_id.to_string(), None)
        .await?;

    Ok(())
}
