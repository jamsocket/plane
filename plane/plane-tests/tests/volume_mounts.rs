use crate::common::test_env::TestEnvironment;
use crate::common::wait_until_backend_terminated;
use plane_common::types::{
    ConnectRequest, DockerExecutorConfig, DronePoolName, KeyConfig, Mount, PullPolicy,
    ResourceLimits, SpawnConfig,
};
use plane_test_macro::plane_test;
use serde_json::Map;
use std::collections::HashMap;
use std::path::PathBuf;

mod common;

#[plane_test]
async fn volume_mounts(env: TestEnvironment) {
    let mount_base = env.scratch_dir.clone();
    let mount = "test_custom_mount";
    let key = "test_key_mount";
    let custom_mount_path = mount_base.join(mount);
    let key_mount_path = mount_base.join(key);

    assert!(mount_base.exists());
    assert!(!custom_mount_path.exists());
    assert!(!key_mount_path.exists());

    let controller = env.controller().await;
    let client = controller.client();
    let _drone = env.drone_with_mount_base(&controller, &mount_base).await;

    // Wait for the drone to register. TODO: this seems long.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    tracing::info!("Requesting backend with custom mount.");
    let connect_request_custom_mount = ConnectRequest {
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
                mount: Some(Mount::Path(PathBuf::from(mount))),
                network_name: None,
            })
            .unwrap(),
            lifetime_limit_seconds: Some(5),
            max_idle_seconds: None,
            use_static_token: false,
            subdomain: None,
        }),
        key: None,
        user: None,
        auth: Map::default(),
    };

    let response_custom_mount = client.connect(&connect_request_custom_mount).await.unwrap();
    tracing::info!("Got response for custom mount.");
    assert!(response_custom_mount.spawned);

    tracing::info!("Requesting backend with key mount.");

    let connect_request_key_mount = ConnectRequest {
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
                mount: Some(Mount::Bool(true)),
                network_name: None,
            })
            .unwrap(),
            lifetime_limit_seconds: Some(5),
            max_idle_seconds: None,
            use_static_token: false,
            subdomain: None,
        }),
        key: Some(KeyConfig {
            name: key.to_string(),
            namespace: "".to_string(),
            tag: "".to_string(),
        }),
        user: None,
        auth: Map::default(),
    };

    let response_key_mount = client.connect(&connect_request_key_mount).await.unwrap();
    tracing::info!("Got response for key mount.");
    assert!(response_key_mount.spawned);

    // Wait for docker to create the folders. TODO: this seems long.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Ensure the mounted folders are created
    assert!(custom_mount_path.exists());
    assert!(key_mount_path.exists());

    // Wait for the backends to terminate
    tracing::info!("Waiting for backends to terminate.");
    wait_until_backend_terminated(&client, &response_custom_mount.backend_id).await;
    wait_until_backend_terminated(&client, &response_key_mount.backend_id).await;
    tracing::info!("Backends terminated.");
}
