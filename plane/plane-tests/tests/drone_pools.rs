use crate::common::wait_until_backend_terminated;
use common::test_env::TestEnvironment;
use plane::types::{
    ConnectRequest, DockerExecutorConfig, DronePoolName, KeyConfig, PullPolicy, ResourceLimits,
    SpawnConfig,
};
use plane_test_macro::plane_test;
use std::collections::HashMap;

mod common;

#[plane_test]
async fn drone_pools(env: TestEnvironment) {
    let controller = env.controller().await;
    let client = controller.client();
    let pool = DronePoolName::from("test");
    let drone = env.drone(&controller).await;
    let drone_in_pool = env.drone_in_pool(&controller, &pool).await;
    assert_ne!(drone.id, drone_in_pool.id);

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
        ..Default::default()
    };

    let response = client.connect(&connect_request).await.unwrap();
    tracing::info!("Got response.");

    assert!(response.spawned);
    let response_drone = response.drone.unwrap().clone();
    assert_eq!(response_drone, drone.id);

    tracing::info!("Requesting backend from pool.");
    let connect_request_with_pool = ConnectRequest {
        spawn_config: Some(SpawnConfig {
            pool,
            ..connect_request.spawn_config.unwrap()
        }),
        key: Some(KeyConfig {
            name: "pool-specific-key".to_string(),
            ..connect_request.key.unwrap()
        }),
        ..Default::default()
    };

    let response_from_pool = client.connect(&connect_request_with_pool).await.unwrap();
    tracing::info!("Got response from pool.");

    assert!(response_from_pool.spawned);
    let response_from_pool_drone = response_from_pool.drone.unwrap().clone();
    assert_eq!(response_from_pool_drone, drone_in_pool.id);

    tracing::info!("Requesting backend from pool with different key.");
    let mut connect_request_with_pool_different_key = connect_request_with_pool.clone();
    connect_request_with_pool_different_key.key = Some(KeyConfig {
        name: "different-key".to_string(),
        ..connect_request_with_pool.key.unwrap()
    });

    let response_from_pool_different_key = client
        .connect(&connect_request_with_pool_different_key)
        .await
        .unwrap();
    tracing::info!("Got response from pool with different key.");

    assert!(response_from_pool_different_key.spawned);
    let response_from_pool_different_key_drone =
        response_from_pool_different_key.drone.unwrap().clone();
    assert_eq!(response_from_pool_different_key_drone, drone_in_pool.id);

    // Additional sanity
    assert_ne!(response_from_pool_drone, response_drone);
    assert_ne!(response_from_pool_different_key_drone, response_drone);

    tracing::info!("Waiting for all backends to terminate.");
    wait_until_backend_terminated(&client, &response.backend_id).await;
    wait_until_backend_terminated(&client, &response_from_pool.backend_id).await;
    wait_until_backend_terminated(&client, &response_from_pool_different_key.backend_id).await;
    tracing::info!("All backends terminated.");
}
