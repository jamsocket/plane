use std::sync::Arc;

use common::test_env::TestEnvironment;
use common::timeout::WithTimeout;
use futures_util::StreamExt;
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

    let runtime = Arc::new(runtime);

    let backend_name = BackendName::new_random();

    {
        // TODO: metrics currently only works if events are being polled for the executor!
        // This is always true in Plane, but is kind of an awkward coupling when it comes to tests.
        // We should refactor the event loop.

        let runtime = runtime.clone();
        tokio::spawn(async move {
            let mut events = runtime.events();

            loop {
                let event = events.next().await.unwrap();
                tracing::info!("Event: {:?}", event);
            }
        });
    }

    runtime
        .spawn(&backend_name, executor_config, None, None)
        .await
        .unwrap();

    tracing::info!("Waiting for metrics.");
    let metrics = recv.recv().with_timeout(60).await.unwrap().unwrap();
    tracing::info!(?metrics, "Received metrics.");
    assert_ne!(metrics.mem_used, 0);
}
