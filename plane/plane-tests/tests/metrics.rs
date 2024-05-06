use common::test_env::TestEnvironment;
use plane::{
    drone::docker::{
        get_metrics_message_from_container_stats, types::ContainerId, MetricsConversionError,
        PlaneDocker, PlaneDockerConfig,
    },
    names::{BackendName, Name},
    types::DockerExecutorConfig,
};
use plane_test_macro::plane_test;
use std::sync::atomic::{AtomicU64, Ordering};

mod common;

#[plane_test]
async fn test_get_metrics(_: TestEnvironment) {
    let plane_docker = PlaneDocker::new(PlaneDockerConfig::default())
        .await
        .unwrap();

    // TODO: replace with locally built hello world
    plane_docker
        .pull(
            "ghcr.io/drifting-in-space/demo-image-drop-four",
            None,
            false,
        )
        .await
        .unwrap();

    let backend_name = BackendName::new_random();
    let container_id = ContainerId::from(format!("plane-test-{}", backend_name));
    let executor_config = DockerExecutorConfig::from_image_with_defaults(
        "ghcr.io/drifting-in-space/demo-image-drop-four",
    );
    plane_docker
        .spawn_backend(&backend_name, &container_id, executor_config, None, None)
        .await
        .unwrap();

    let metrics = plane_docker.get_metrics(&container_id).await;
    assert!(metrics.is_ok());
    let mut metrics = metrics.unwrap();
    let prev_container_cpu = AtomicU64::new(0);
    let prev_sys_cpu = AtomicU64::new(0);

    let backend_metrics_message = get_metrics_message_from_container_stats(
        metrics.clone(),
        backend_name.clone(),
        &prev_sys_cpu,
        &prev_container_cpu,
    );

    assert!(backend_metrics_message.is_ok());

    let tmp_mem = metrics.memory_stats.usage.clone();
    metrics.memory_stats.usage = None;

    let backend_metrics_message = get_metrics_message_from_container_stats(
        metrics.clone(),
        backend_name.clone(),
        &prev_sys_cpu,
        &prev_container_cpu,
    );

    assert!(matches!(
        backend_metrics_message,
        Err(MetricsConversionError::NoStatsAvailable(_))
    ));

    metrics.memory_stats.usage = tmp_mem;
    let tmp_mem = metrics.memory_stats.stats;
    metrics.memory_stats.stats = None;

    let backend_metrics_message = get_metrics_message_from_container_stats(
        metrics.clone(),
        backend_name.clone(),
        &prev_sys_cpu,
        &prev_container_cpu,
    );

    assert!(matches!(
        backend_metrics_message,
        Err(MetricsConversionError::NoStatsAvailable(_))
    ));
    metrics.memory_stats.stats = tmp_mem;

    let tmp_mem = metrics.cpu_stats.system_cpu_usage;
    metrics.cpu_stats.system_cpu_usage = None;

    let backend_metrics_message = get_metrics_message_from_container_stats(
        metrics.clone(),
        backend_name.clone(),
        &prev_sys_cpu,
        &prev_container_cpu,
    );

    assert!(matches!(
        backend_metrics_message,
        Err(MetricsConversionError::NoStatsAvailable(_))
    ));
    metrics.cpu_stats.system_cpu_usage = tmp_mem;

    prev_sys_cpu.store(u64::MAX, Ordering::SeqCst);
    let backend_metrics_message = get_metrics_message_from_container_stats(
        metrics.clone(),
        backend_name.clone(),
        &prev_sys_cpu,
        &prev_container_cpu,
    );

    assert!(matches!(
        backend_metrics_message,
        Err(MetricsConversionError::SysCpuLessThanCurrent { .. })
    ));
    prev_sys_cpu.store(0, Ordering::SeqCst);

    prev_container_cpu.store(u64::MAX, Ordering::SeqCst);
    let backend_metrics_message = get_metrics_message_from_container_stats(
        metrics.clone(),
        backend_name.clone(),
        &prev_sys_cpu,
        &prev_container_cpu,
    );

    assert!(matches!(
        backend_metrics_message,
        Err(MetricsConversionError::ContainerCpuLessThanCurrent { .. })
    ));
    prev_container_cpu.store(0, Ordering::SeqCst);
}
