use crate::common::timeout::WithTimeout;
use common::test_env::TestEnvironment;
use plane::{
    names::{Name, ProxyName},
    protocol::{MessageFromProxy, MessageToProxy, RouteInfoRequest, RouteInfoResponse},
    types::{BackendStatus, ConnectRequest, DronePoolName, ExecutorConfig, PullPolicy, ResourceLimits, SpawnConfig},
};
use plane_test_macro::plane_test;
use serde_json::Map;
use std::collections::HashMap;

mod common;

#[plane_test]
async fn backend_lifecycle(env: TestEnvironment) {
    let db = env.db().await;
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
            executable: ExecutorConfig {
                image: "ghcr.io/drifting-in-space/demo-image-drop-four".to_string(),
                pull_policy: Some(PullPolicy::IfNotPresent),
                env: HashMap::default(),
                resource_limits: ResourceLimits::default(),
                credentials: None,
                mount: None,
            },
            lifetime_limit_seconds: None,
            max_idle_seconds: None,
            use_static_token: false,
            subdomain: None,
        }),
        key: None,
        user: None,
        auth: Map::default(),
    };
    let response = client.connect(&connect_request).await.unwrap();
    tracing::info!("Got response.");

    assert!(response.spawned);

    let backend_id = response.backend_id.clone();

    tracing::info!("Streaming status.");
    let mut backend_status_stream = client
        .backend_status_stream(&backend_id)
        .with_timeout(10)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Scheduled,
    );
    tracing::info!("Got scheduled status.");

    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Loading,
    );
    tracing::info!("Got loading status.");

    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Starting,
    );
    tracing::info!("Got starting status.");

    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Waiting,
    );
    tracing::info!("Got waiting status.");

    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Ready,
    );
    tracing::info!("Got ready status.");

    // Test non-streaming status endpoint.
    let status = client
        .backend_status(&response.backend_id)
        .with_timeout(10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, BackendStatus::Ready);
    tracing::info!("Got non-streaming ready status.");

    let proxy_connection = client.proxy_connection(&env.cluster);
    tracing::info!("Connecting as proxy.");
    let mut proxy = proxy_connection
        .connect(&ProxyName::new_random())
        .await
        .unwrap();
    tracing::info!("Connected as proxy. Requesting route info.");

    proxy
        .send(MessageFromProxy::RouteInfoRequest(RouteInfoRequest {
            token: response.token.clone(),
        }))
        .await
        .unwrap();

    let result = proxy.recv().with_timeout(10).await.unwrap().unwrap();
    tracing::info!("Got route info response.");

    let MessageToProxy::RouteInfoResponse(RouteInfoResponse { token, route_info }) = result else {
        panic!("Unexpected message: {:?}", result);
    };

    assert_eq!(token, response.token);
    assert_eq!(
        route_info.unwrap().secret_token,
        response.secret_token.unwrap()
    );

    tracing::info!("Getting last keepalive time.");
    let initial_keepalive = {
        let backend = db
            .backend()
            .backend(&response.backend_id)
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        backend.last_keepalive
    };

    tracing::info!("Sending keepalive.");
    proxy
        .send(MessageFromProxy::KeepAlive(response.backend_id.clone()))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    tracing::info!("Getting last keepalive time again.");
    {
        let backend = db
            .backend()
            .backend(&response.backend_id)
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert!(backend.last_keepalive > initial_keepalive);
    }

    tracing::info!("Terminating backend.");
    client
        .soft_terminate(&response.backend_id)
        .with_timeout(10)
        .await
        .unwrap()
        .unwrap();

    tracing::info!("Waiting for terminating status.");
    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Terminating,
    );
    tracing::info!("Got terminating status.");

    assert_eq!(
        backend_status_stream
            .next()
            .with_timeout(10)
            .await
            .unwrap()
            .unwrap()
            .status,
        BackendStatus::Terminated,
    );
    tracing::info!("Got terminated status.");
}
