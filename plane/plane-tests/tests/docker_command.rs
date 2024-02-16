use anyhow::Context;
use bollard::container::LogsOptions;
use futures_util::TryStreamExt;
use plane::{
    drone::docker::{
        commands::{get_container_config_from_executor_config, pull_image},
        types::ContainerId,
    },
    names::{BackendName, Name},
    types::{ExecutorConfig, ResourceLimits},
};
use serde_json;

/// starts and runs command in container
/// with given container config, returns stdout + stderr as string
/// handles pulling image and removes container after it's done.
async fn run_container_with_config(
    config: bollard::container::Config<String>,
) -> anyhow::Result<String> {
    let docker = bollard::Docker::connect_with_local_defaults().context("Connecting to Docker")?;
    let image = config.image.clone().unwrap();
    pull_image(&docker, &image, None, false)
        .await
        .context("Pulling image")?;
    let backend_name = BackendName::new_random();
    let container_id = ContainerId::from(format!("plane-test-{}", backend_name));
    let create_container_options = bollard::container::CreateContainerOptions {
        name: container_id.to_string(),
        ..Default::default()
    };

    docker
        .create_container(Some(create_container_options), config)
        .await
        .context("Creating container")?;
    docker
        .start_container::<String>(&container_id.to_string(), None)
        .await
        .context("Starting container")?;
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
        .await
        .context("Removing container")?;

    Ok(output)
}

#[tokio::test]
async fn test_resource_limits() {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let backend_name = BackendName::new_random();
    let _container_id = ContainerId::from(format!("plane-test-{}", backend_name));
    let mut executor_config = ExecutorConfig::from_image_with_defaults("debian:bookworm");

    let resource_limits: ResourceLimits = serde_json::from_value(serde_json::json!( {
        "cpu_period": 1000000,
        "cpu_time_limit": 100,
        "cpu_period_percent": 80,
        "memory_limit_bytes": 10000000,
        "disk_limit_bytes": 10000000000 as i64
    }))
    .unwrap();
    executor_config.resource_limits = resource_limits;
    let info = docker.info().await.unwrap();
    match info.driver {
        Some(t) if matches!(t.as_str(), "btrfs" | "zfs") => {}
        Some(t)
            if t == "overlay2" && {
                if let Some(status) = info.driver_status {
                    status.iter().flatten().any(|s| s.contains("xfs"))
                } else {
                    true
                }
            } =>
        {
            //disk limits not supported with xfs backend for overlay 2
            executor_config.resource_limits.disk_limit_bytes = None;
        }
        _ => {
            //disk limits not supported otherwise
            executor_config.resource_limits.disk_limit_bytes = None;
        }
    }

    let mut config = get_container_config_from_executor_config(
        &backend_name,
        executor_config.clone(),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    config.cmd = Some(vec!["echo".into(), "hello world".into()]);
    let out = run_container_with_config(config.clone()).await.unwrap();
    assert_eq!(out.trim(), "hello world".to_string());

    config.cmd = None;
    config.entrypoint = Some(vec!["sh".into(), "-c".into(), "ulimit -t".into()]);
    let out = run_container_with_config(config.clone()).await.unwrap();
    assert_eq!(out.trim(), "100".to_string());

    config.entrypoint = None;
    config.cmd = Some(vec!["cat".into(), "/sys/fs/cgroup/cpu.max".into()]);
    let out = run_container_with_config(config.clone()).await.unwrap();
    assert_eq!(out.trim(), "800000 1000000".to_string());

    //NOTE: memory limit rounds down to nearest page size multiple,
    // this assumes the page size to be 4096 bytes.
    let mem_limit_corr = 10000000 - (10000000 % 4096);
    config.cmd = Some(vec!["cat".into(), "/sys/fs/cgroup/memory.max".into()]);
    let out = run_container_with_config(config.clone()).await.unwrap();
    assert_eq!(out.trim(), mem_limit_corr.to_string());

    //unfortunately, the docker disk limit is dependent on the storage backend
    //hence just validating here that config is as expected.
    if let Some(expected_size) = executor_config.resource_limits.disk_limit_bytes {
        assert_eq!(
            expected_size.to_string(),
            config
                .host_config
                .unwrap()
                .storage_opt
                .unwrap()
                .get("size")
                .unwrap()
                .clone()
        );
    }
}
