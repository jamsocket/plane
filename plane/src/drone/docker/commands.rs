use super::{types::ContainerId, PlaneDocker};
use crate::{
    names::BackendName,
    protocol::AcquiredKey,
    types::{BearerToken, ExecutorConfig, Mount},
};
use anyhow::Result;
use bollard::{
    auth::DockerCredentials,
    service::{HostConfig, HostConfigLogConfig, PortBinding, ResourcesUlimits},
    Docker,
};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    time::Duration,
};

/// Port inside the container to expose.
const CONTAINER_PORT: u16 = 8080;
/// Base directory for any data mounted in the container from the host
const PLANE_DATA_DIR: &str = "/plane-data";

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
            tracing::debug!(?progress, "Image pull progress.");
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

// Disallow absolute paths or paths with dots (.. or .)
pub fn validate_mount_path(path: &Path) -> Result<()> {
    for component in path.components() {
        match component {
            Component::Prefix(..) | Component::Normal(..) => continue,
            _ => {
                return Err(anyhow::anyhow!(
                "Spawn request contains invalid mount path, {:?}, that is not under the mount base",
                path
            ))
            }
        }
    }
    Ok(())
}

pub fn get_container_config_from_executor_config(
    backend_id: &BackendName,
    exec_config: ExecutorConfig,
    runtime: Option<&str>,
    key: Option<&AcquiredKey>,
    static_token: Option<&BearerToken>,
    log_config: Option<&HostConfigLogConfig>,
    mount_base: Option<&PathBuf>,
) -> Result<bollard::container::Config<String>> {
    let mut env = exec_config.env;
    env.insert("PORT".to_string(), CONTAINER_PORT.to_string());
    env.insert("SESSION_BACKEND_ID".to_string(), backend_id.to_string());

    if let Some(key) = key {
        env.insert(
            "SESSION_BACKEND_FENCING_TOKEN".to_string(),
            key.token.to_string(),
        );
        env.insert("SESSION_BACKEND_KEY".to_string(), key.key.name.to_string());
    }

    if let Some(static_token) = static_token {
        env.insert(
            "SESSION_BACKEND_STATIC_TOKEN".to_string(),
            static_token.to_string(),
        );
    }

    // TODO: set PLANE_LOCK and PLANE_FENCING_TOKEN.
    let env: Vec<String> = env
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    let binds: Option<Vec<String>> = match (&mount_base, &exec_config.mount) {
        (_, None) | (_, Some(Mount::Bool(false))) => None,
        (Some(base), Some(mount_option)) => {
            let mount_path = match mount_option {
                Mount::Bool(true) => {
                    if let Some(key) = key {
                        base.join(&key.key.name)
                    } else {
                        return Err(anyhow::anyhow!(
                            "Key is required for Bool(true) mount option"
                        ));
                    }
                }
                Mount::Path(path) => {
                    validate_mount_path(path)?;
                    base.join(path)
                }
                Mount::Bool(false) => unreachable!(),
            };
            Some(vec![format!(
                "{}:{}",
                mount_path.to_string_lossy(),
                PLANE_DATA_DIR
            )])
        }
        (None, Some(mount)) => {
            tracing::warn!(
                "Spawn request included a mount: {:?}, but drone has no mount base. Mounting will not be performed.",
                mount
            );
            None
        }
    };

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
            runtime: runtime.map(|s| s.to_string()),
            memory: exec_config.resource_limits.memory_limit_bytes,
            log_config: log_config.cloned(),
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
                    let Ok(secs) = cpu_time_limit.0.as_secs().try_into() else {
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
            binds,
            ..Default::default()
        }),
        ..Default::default()
    })
}

pub async fn run_container(
    docker: &PlaneDocker,
    backend_id: &BackendName,
    container_id: &ContainerId,
    exec_config: ExecutorConfig,
    acquired_key: Option<&AcquiredKey>,
    static_token: Option<&BearerToken>,
) -> Result<()> {
    let options = bollard::container::CreateContainerOptions {
        name: container_id.to_string(),
        ..Default::default()
    };

    let config = get_container_config_from_executor_config(
        backend_id,
        exec_config,
        docker.config.runtime.as_deref(),
        acquired_key,
        static_token,
        docker.config.log_config.as_ref(),
        docker.config.mount_base.as_ref(),
    )?;

    docker
        .docker
        .create_container(Some(options), config)
        .await?;

    docker
        .docker
        .start_container::<String>(&container_id.to_string(), None)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        log_types::LoggableTime,
        names::Name,
        protocol::{AcquiredKey, KeyDeadlines},
        types::{ExecutorConfig, KeyConfig, Mount},
    };
    use std::time::UNIX_EPOCH;

    fn get_container_config_from_mount(
        mount_base: &str,
        mount: Option<Mount>,
    ) -> Result<bollard::container::Config<String>> {
        let backend_name = BackendName::new_random();
        let key = "key";

        let acquired_key = Some(AcquiredKey {
            key: KeyConfig {
                name: key.to_string(),
                ..Default::default()
            },
            deadlines: KeyDeadlines {
                renew_at: LoggableTime(UNIX_EPOCH.into()),
                soft_terminate_at: LoggableTime(UNIX_EPOCH.into()),
                hard_terminate_at: LoggableTime(UNIX_EPOCH.into()),
            },
            token: Default::default(),
        });

        let mut exec_config = ExecutorConfig::from_image_with_defaults(String::default());
        exec_config.mount = mount;

        get_container_config_from_executor_config(
            &backend_name,
            exec_config,
            None,
            acquired_key.as_ref(),
            None,
            None,
            Some(&PathBuf::from(mount_base)),
        )
    }

    // Test basic mount options

    #[test]
    fn test_mount_true() {
        let mount_base = "/mnt/my-nfs";
        let mount = Some(Mount::Bool(true));
        let expected_binds = Some(vec![format!("/mnt/my-nfs/key:{}", PLANE_DATA_DIR)]);

        let config = get_container_config_from_mount(mount_base, mount)
            .unwrap()
            .host_config
            .unwrap()
            .binds;

        assert_eq!(config, expected_binds);
    }

    #[test]
    fn test_mount_path() {
        let mount_base = "/mnt/my-nfs";
        let mount = Some(Mount::Path(PathBuf::from("custom-mount")));
        let expected_binds = Some(vec![format!("/mnt/my-nfs/custom-mount:{}", PLANE_DATA_DIR)]);

        let config = get_container_config_from_mount(mount_base, mount)
            .unwrap()
            .host_config
            .unwrap()
            .binds;

        assert_eq!(config, expected_binds);
    }

    #[test]
    fn test_mount_false() {
        let mount_base = "/mnt/my-nfs";
        let mount = Some(Mount::Bool(false));
        let expected_binds = None;

        let config = get_container_config_from_mount(mount_base, mount)
            .unwrap()
            .host_config
            .unwrap()
            .binds;

        assert_eq!(config, expected_binds);
    }

    #[test]
    fn test_mount_none() {
        let mount_base = "/mnt/my-nfs";
        let mount = None;
        let expected_binds = None;

        let config = get_container_config_from_mount(mount_base, mount)
            .unwrap()
            .host_config
            .unwrap()
            .binds;

        assert_eq!(config, expected_binds);
    }

    // Test invalid mount paths

    #[test]
    fn test_mount_path_invalid_parent() {
        let mount_base = "/mnt/my-nfs";
        let mount = Some(Mount::Path(PathBuf::from("..")));

        let err = get_container_config_from_mount(mount_base, mount).unwrap_err();
        assert!(err.to_string().contains("not under the mount base"));
    }

    #[test]
    fn test_mount_path_invalid_sibling() {
        let mount_base = "/mnt/my-nfs";
        let mount = Some(Mount::Path(PathBuf::from("../different-folder")));

        let err = get_container_config_from_mount(mount_base, mount).unwrap_err();
        assert!(err.to_string().contains("not under the mount base"));
    }

    #[test]
    fn test_mount_path_invalid_complex_escape() {
        let mount_base = "/mnt/my-nfs";
        let mount = Some(Mount::Path(PathBuf::from("hide/under/../../../escape")));

        let err = get_container_config_from_mount(mount_base, mount).unwrap_err();
        assert!(err.to_string().contains("not under the mount base"));
    }
}
