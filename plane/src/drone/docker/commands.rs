use super::types::ContainerId;
use crate::{names::BackendName, types::ExecutorConfig};
use anyhow::Result;
use bollard::{
    auth::DockerCredentials,
    service::{HostConfig, PortBinding, ResourcesUlimits},
    Docker,
};
use futures_util::StreamExt;
use std::{collections::HashMap, time::Duration};

/// Port inside the container to expose.
const CONTAINER_PORT: u16 = 8080;

pub async fn pull_image(
    docker: &Docker,
    image: &str,
    credentials: Option<&DockerCredentials>,
    force: bool,
) -> Result<()> {
    if !force && image_exists(docker, image).await? {
        tracing::info!(?image, "Skipping image that already exists.");
        return Ok(());
    }

    let options = bollard::image::CreateImageOptions {
        from_image: image,
        ..Default::default()
    };

    let mut result = docker.create_image(Some(options), None, credentials.cloned());
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
        .and_then(|bindings| bindings.first().cloned())
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

fn get_container_config_from_executor_config(
    backend_id: &BackendName,
    exec_config: ExecutorConfig,
    runtime: &Option<String>,
) -> Result<bollard::container::Config<String>> {
    let mut env = exec_config.env;
    env.insert("PORT".to_string(), CONTAINER_PORT.to_string());
    env.insert("PLANE_BACKEND_ID".to_string(), backend_id.to_string());
    // TODO: set PLANE_LOCK and PLANE_FENCING_TOKEN.
    let env: Vec<String> = env
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    Ok(bollard::container::Config {
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
                    let Ok(secs) = cpu_time_limit.as_secs().try_into() else {
                        tracing::warn!(
                            "unable to convert cpu_time_limit: {:?} to i64 for use in ulimit",
                            cpu_time_limit
                        );
                        return vec![];
                    };
                    vec![ResourcesUlimits {
                        name: Some("cpu".to_string()),
                        soft: Some(secs),
                        hard: Some(secs),
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
    })
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

    let config = get_container_config_from_executor_config(backend_id, exec_config, runtime)?;

    docker.create_container(Some(options), config).await?;

    docker
        .start_container::<String>(&container_id.to_string(), None)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{names::Name, types::ResourceLimits};
    use bollard::container::LogsOptions;
    use futures_util::TryStreamExt;
    use serde_json;

    use super::*;

    /// starts and runs command in container
    /// with given container config, returns stdout + stderr as string
    /// handles pulling image and removes container after it's done.
    async fn run_container_with_config(
        config: bollard::container::Config<String>,
    ) -> anyhow::Result<String> {
        let docker = bollard::Docker::connect_with_local_defaults()?;
        let image = config.image.clone().unwrap();
        pull_image(&docker, &image, None, false).await?;
        let backend_name = BackendName::new_random();
        let container_id = ContainerId::from(format!("plane-test-{}", backend_name));
        let create_container_options = bollard::container::CreateContainerOptions {
            name: container_id.to_string(),
            ..Default::default()
        };

        docker
            .create_container(Some(create_container_options), config)
            .await?;
        docker
            .start_container::<String>(&container_id.to_string(), None)
            .await?;
        let mut wait_container = docker.wait_container::<String>(&container_id.to_string(), None);

        let mut output = String::new();
        while let (Some(msg), Some(_)) = (
            docker
                .logs::<String>(
                    &container_id.to_string(),
                    Some(LogsOptions {
                        follow: true,
                        stdout: true,
                        stderr: true,
                        ..Default::default()
                    }),
                )
                .try_next()
                .await?,
            wait_container.try_next().await?,
        ) {
            output = output + &msg.to_string();
        }

        docker
            .remove_container(&container_id.to_string(), None)
            .await?;

        Ok(output)
    }

    #[tokio::test]
    async fn test_resource_limits() -> anyhow::Result<()> {
        let docker = bollard::Docker::connect_with_local_defaults()?;
        let backend_name = BackendName::new_random();
        let _container_id = ContainerId::from(format!("plane-test-{}", backend_name));
        let mut executor_config = ExecutorConfig::from_image_with_defaults("debian:bookworm");

        let resource_limits: ResourceLimits = serde_json::from_str(
            r#"
        {
        "cpu_period": 1000000,
        "cpu_time_limit": 100,
        "cpu_period_percent": 80,
        "memory_limit_bytes": 10000000,
        "disk_limit_bytes": 10000000000
        }
        "#,
        )?;

        executor_config.resource_limits = resource_limits;
        match docker.info().await?.driver {
            Some(t) if matches!(t.as_str(), "btrfs" | "zfs" | "overlay2") => {}
            _ => {
                //disk limits not supported otherwise
                executor_config.resource_limits.disk_limit_bytes = None;
            }
        }

        let mut config =
            get_container_config_from_executor_config(&backend_name, executor_config, &None)?;
        config.cmd = Some(vec!["echo".into(), "hello world".into()]);
        let out = run_container_with_config(config.clone()).await?;
        assert_eq!(out.trim(), "hello world".to_string());

        config.cmd = None;
        config.entrypoint = Some(vec!["sh".into(), "-c".into(), "ulimit -t".into()]);
        let out = run_container_with_config(config.clone()).await?;
        assert_eq!(out.trim(), "100".to_string());

        config.entrypoint = None;
        config.cmd = Some(vec!["cat".into(), "/sys/fs/cgroup/cpu.max".into()]);
        let out = run_container_with_config(config.clone()).await?;
        assert_eq!(out.trim(), "800000 1000000".to_string());

        //NOTE: memory limit rounds down to nearest page size multiple,
        // this assumes the page size to be 4096 bytes.
        let mem_limit_corr = 10000000 - (10000000 % 4096);
        config.cmd = Some(vec!["cat".into(), "/sys/fs/cgroup/memory.max".into()]);
        let out = run_container_with_config(config.clone()).await?;
        assert_eq!(out.trim(), mem_limit_corr.to_string());

        //unfortunately, the docker disk limit is dependent on the storage backend
        //hence just validating here that config is as expected.
        let size = config
            .host_config
            .unwrap()
            .storage_opt
            .unwrap()
            .get("size")
            .cloned();
        assert_eq!(size, Some("10000000000".to_string()));

        Ok(())
    }
}
