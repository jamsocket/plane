use common::test_env::TestEnvironment;
use common::timeout::WithTimeout;
use plane::{
    drone::{
        docker::{DockerRuntime, DockerRuntimeConfig},
        runtime::Runtime,
    },
    names::{BackendName, Name},
    types::DockerExecutorConfig,
};
use plane_test_macro::plane_test;

mod common;

#[plane_test]
async fn test_get_metrics(_: TestEnvironment) {
    let runtime = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .unwrap();

    let executor_config = DockerExecutorConfig::from_image_with_defaults(
        "ghcr.io/drifting-in-space/demo-image-drop-four",
    );

    runtime.prepare(&executor_config).await.unwrap();
    let (send, mut recv) = tokio::sync::mpsc::channel(1);
    runtime.metrics_callback(move |metrics_message| {
        send.try_send(metrics_message).unwrap();
    });

    let backend_name = BackendName::new_random();

    runtime
        .spawn(&backend_name, executor_config, None, None)
        .await
        .unwrap();

    tracing::info!("Waiting for metrics.");
    let metrics = recv.recv().with_timeout(60).await.unwrap().unwrap();
    tracing::info!(?metrics, "Received metrics.");
    assert_ne!(metrics.mem_used, 0);
}
