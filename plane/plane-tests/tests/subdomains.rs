use crate::common::{test_env::TestEnvironment, wait_until_backend_terminated};
use plane::types::{ConnectRequest, ExecutorConfig, PullPolicy, ResourceLimits, SpawnConfig};
use plane_test_macro::plane_test;
use serde_json::Map;
use std::collections::HashMap;

mod common;

#[plane_test]
async fn subdomains(env: TestEnvironment) {
    let controller = env.controller().await;
    let client = controller.client();
    let _drone = env.drone(&controller).await;

    // Wait for the drone to register. TODO: this seems long.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Connect request without subdomain
    let connect_request = ConnectRequest {
        spawn_config: Some(SpawnConfig {
            id: None,
            cluster: Some(env.cluster.clone()),
            executable: ExecutorConfig {
                image: "ghcr.io/drifting-in-space/demo-image-drop-four".to_string(),
                pull_policy: Some(PullPolicy::IfNotPresent),
                env: HashMap::default(),
                resource_limits: ResourceLimits::default(),
                credentials: None,
                mount: None,
            },
            lifetime_limit_seconds: Some(5),
            max_idle_seconds: None,
            use_static_token: false,
            subdomain: None,
        }),
        key: None,
        user: None,
        auth: Map::default(),
        pool: None,
    };

    // Connect request with subdomain
    let connect_request_with_subdomain = ConnectRequest {
        spawn_config: Some(SpawnConfig {
            subdomain: Some("subdomain".to_string()),
            ..connect_request.spawn_config.clone().unwrap()
        }),
        ..connect_request.clone()
    };

    // Perform connections
    let response = client.connect(&connect_request).await.unwrap();
    let response_with_subdomain = client
        .connect(&connect_request_with_subdomain)
        .await
        .unwrap();

    // Assertions for no subdomain
    let expected_url = format!("https://{}/{}/", env.cluster, response.token);
    assert_eq!(
        response.url, expected_url,
        "URL without subdomain does not match expected format."
    );

    // Assertions for with subdomain
    let expected_subdomain_url = format!(
        "https://subdomain.{}/{}/",
        env.cluster, response_with_subdomain.token
    );
    assert_eq!(
        response_with_subdomain.url, expected_subdomain_url,
        "URL with subdomain does not match expected format."
    );

    // Connect request with "sub.sub" as the subdomain
    let connect_request_with_sub_subdomain = ConnectRequest {
        spawn_config: Some(SpawnConfig {
            subdomain: Some("sub.sub".to_string()),
            ..connect_request.spawn_config.clone().unwrap()
        }),
        ..connect_request.clone()
    };

    // sub-sub domains are not allowed, expect a spawn error
    let error_with_sub_subdomain = client
        .connect(&connect_request_with_sub_subdomain)
        .await
        .expect_err("Expected an error due to invalid subdomain format.");
    assert!(error_with_sub_subdomain
        .to_string()
        .contains("Invalid subdomain provided."));

    wait_until_backend_terminated(&client, &response.backend_id).await;
    wait_until_backend_terminated(&client, &response_with_subdomain.backend_id).await;
}
