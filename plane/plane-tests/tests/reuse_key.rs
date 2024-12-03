use crate::common::wait_until_backend_terminated;
use common::test_env::TestEnvironment;
use plane_common::types::{
    ConnectRequest, DockerExecutorConfig, DronePoolName, KeyConfig, PullPolicy, ResourceLimits,
    SpawnConfig,
};
use plane_test_macro::plane_test;
use serde_json::Map;
use std::collections::HashMap;

mod common;

#[plane_test]
async fn reuse_key(env: TestEnvironment) {
    let controller = env.controller().await;
    let client = controller.client();
    let _drone = env.drone(&controller).await;

    // Wait for the drone to register. TODO: this seems long.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    tracing::info!("Requesting backend.");
    let connect_request = ConnectRequest {
        spawn_config: Some(SpawnConfig {
            id: None,
            cluster: Some(env.cluster.clone()),
            pool: DronePoolName::default(),
            executable: serde_json::to_value(DockerExecutorConfig {
                image: "ghcr.io/jamsocket/demo-image-drop-four".to_string(),
                pull_policy: Some(PullPolicy::IfNotPresent),
                env: HashMap::default(),
                resource_limits: ResourceLimits::default(),
                credentials: None,
                mount: None,
                network_name: None,
            })
            .unwrap(),
            lifetime_limit_seconds: Some(5),
            max_idle_seconds: None,
            use_static_token: false,
            subdomain: None,
        }),
        key: Some(KeyConfig {
            name: "reuse-key".to_string(),
            namespace: "".to_string(),
            tag: "".to_string(),
        }),
        user: None,
        auth: Map::default(),
    };

    let response = client.connect(&connect_request).await.unwrap();
    tracing::info!("Got response.");

    assert!(response.spawned);

    let response2 = client.connect(&connect_request).await.unwrap();

    assert!(!response2.spawned);
    assert_eq!(response2.backend_id, response.backend_id);

    wait_until_backend_terminated(&client, &response.backend_id).await;
}
