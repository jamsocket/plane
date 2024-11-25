use common::test_env::TestEnvironment;
use common::timeout::WithTimeout;
use plane::drone::{
    runtime::docker::{DockerRuntime, DockerRuntimeConfig},
    runtime::Runtime,
};
use plane_client::{
    names::{BackendName, Name},
    types::DockerExecutorConfig,
};
use plane_test_macro::plane_test;
use serde_json::json;

mod common;

#[plane_test]
async fn test_get_metrics(_: TestEnvironment) {
    let runtime = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .unwrap();

    let executor_config = json!(DockerExecutorConfig::from_image_with_defaults(
        "ghcr.io/jamsocket/demo-image-drop-four"
    ));

    runtime.prepare(&executor_config).await.unwrap();
    let (send, mut recv) = tokio::sync::mpsc::channel(1);
    runtime.metrics_callback(Box::new(move |metrics_message| {
        send.try_send(metrics_message).unwrap();
    }));

    let backend_name = BackendName::new_random();

    runtime
        .spawn(&backend_name, &executor_config, None, None)
        .await
        .unwrap();

    tracing::info!("Waiting for metrics.");
    let metrics = recv.recv().with_timeout(60).await.unwrap().unwrap();
    tracing::info!(?metrics, "Received metrics.");
    assert_ne!(metrics.mem_used, 0);
}
