use crate::common::test_env::TestEnvironment;
use plane::types::{
    ClusterName, ConnectRequest, ExecutorConfig, PullPolicy, ResourceLimits, SpawnConfig,
};
use plane_test_macro::plane_test;
use reqwest::{StatusCode, Url};
use serde_json::Map;
use std::collections::HashMap;

mod common;

#[plane_test]
async fn subdomains(env: TestEnvironment) {
    println!("Starting test");
    let cluster: ClusterName = "localhost:9090".parse().unwrap();

    let controller = env.controller().await;
    let client = controller.client();
    let _drone = env
        .drone_internal(&controller, Some(&cluster), "", None)
        .await;
    println!("Starting proxy");
    let _proxy = env.proxy(&controller).await;
    println!("Proxy started successfully");

    // Wait for the drone to register. TODO: this seems long.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Connect request without subdomain
    let connect_request = ConnectRequest {
        spawn_config: Some(SpawnConfig {
            id: None,
            cluster: Some(cluster.clone()),
            executable: ExecutorConfig {
                image: "ghcr.io/drifting-in-space/demo-image-drop-four".to_string(),
                pull_policy: Some(PullPolicy::IfNotPresent),
                env: HashMap::default(),
                resource_limits: ResourceLimits::default(),
                credentials: None,
                mount: None,
            },
            // lifetime_limit_seconds: Some(5),
            lifetime_limit_seconds: None,
            max_idle_seconds: None,
            use_static_token: false,
            subdomain: None,
        }),
        key: None,
        user: None,
        auth: Map::default(),
        pool: None,
    };

    // // Connect request with subdomain
    // let connect_request_with_subdomain = ConnectRequest {
    //     spawn_config: Some(SpawnConfig {
    //         subdomain: Some("subdomain".to_string()),
    //         ..connect_request.spawn_config.clone().unwrap()
    //     }),
    //     ..connect_request.clone()
    // };

    // Perform connections
    let response = client.connect(&connect_request).await.unwrap();
    // let response_with_subdomain = client
    //     .connect(&connect_request_with_subdomain)
    //     .await
    //     .unwrap();
    // Wait for the drone to register. TODO: this seems long.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("Got response with url: {}", response.url);
    // HTTP client setup
    let http_client = reqwest::Client::new();

    // Assertions for no subdomain
    let url = Url::parse(&response.url).unwrap(); // Parse the URL to manipulate it
    let domain = url.host_str().unwrap(); // Extract the domain
    let scheme = url.scheme(); // Extract the scheme (http or https)
    println!("Extracted domain: {}, scheme: {}", domain, scheme);
    let res = http_client.get(&response.url).send().await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Full HTTP response: {:?}",
        res
    );
    //     http_client
    //         .get(format!("{}://sub.{}", scheme, domain))
    //         .send()
    //         .await
    //         .unwrap()
    //         .status(),
    //     StatusCode::UNAUTHORIZED
    // );
    // assert_eq!(
    //     http_client
    //         .get(format!("{}://sub.sub.{}", scheme, domain))
    //         .send()
    //         .await
    //         .unwrap()
    //         .status(),
    //     StatusCode::UNAUTHORIZED
    // );

    // // Assertions for with subdomain
    // let subdomain_url = Url::parse(&response_with_subdomain.url).unwrap(); // Parse the URL
    // let subdomain_domain = subdomain_url.host_str().unwrap(); // Extract the domain
    // let subdomain_scheme = subdomain_url.scheme(); // Extract the scheme
    // tracing::info!(
    //     "Extracted subdomain domain: {}, scheme: {}",
    //     subdomain_domain,
    //     subdomain_scheme
    // );
    // assert_eq!(
    //     http_client
    //         .get(&response_with_subdomain.url)
    //         .send()
    //         .await
    //         .unwrap()
    //         .status(),
    //     StatusCode::OK
    // );
    // assert_eq!(
    //     http_client
    //         .get(format!(
    //             "{}://different.{}",
    //             subdomain_scheme, subdomain_domain
    //         ))
    //         .send()
    //         .await
    //         .unwrap()
    //         .status(),
    //     StatusCode::UNAUTHORIZED
    // );
    // assert_eq!(
    //     http_client
    //         .get(format!(
    //             "{}://subdomain.{}",
    //             subdomain_scheme, subdomain_domain
    //         ))
    //         .send()
    //         .await
    //         .unwrap()
    //         .status(),
    //     StatusCode::UNAUTHORIZED
    // );
    // assert_eq!(
    //     http_client
    //         .get(format!(
    //             "{}://subdomain.sub.{}",
    //             subdomain_scheme, subdomain_domain
    //         ))
    //         .send()
    //         .await
    //         .unwrap()
    //         .status(),
    //     StatusCode::UNAUTHORIZED
    // );
}
