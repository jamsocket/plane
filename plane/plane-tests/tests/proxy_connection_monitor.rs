use common::{
    localhost_resolver::localhost_client, proxy_mock::MockProxy,
    simple_axum_server::SimpleAxumServer, test_env::TestEnvironment,
    websocket_echo_server::WebSocketEchoServer,
};
use plane::{
    log_types::BackendAddr,
    names::{BackendName, Name},
    protocol::{RouteInfo, RouteInfoResponse},
    types::{BearerToken, ClusterName, SecretToken},
};
use plane_test_macro::plane_test;
use reqwest::StatusCode;
use std::str::FromStr;
use tokio_tungstenite::connect_async;

mod common;

#[plane_test]
async fn proxy_marks_backend_as_recently_active(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;
    let backend_name = BackendName::new_random();

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
    let url = format!("http://plane.test:{port}/abc123/");
    let client = localhost_client();

    let backend_entry = proxy.backend_entry(&backend_name);
    assert!(backend_entry.is_none());

    let handle = tokio::spawn(client.get(url).send());
    let route_info_request = proxy.recv_route_info_request().await;
    assert_eq!(
        route_info_request.token,
        BearerToken::from("abc123".to_string())
    );
    println!("received route info request");

    proxy
        .send_route_info_response(RouteInfoResponse {
            token: BearerToken::from("abc123".to_string()),
            route_info: Some(RouteInfo {
                backend_id: backend_name.clone(),
                address: BackendAddr(server.addr()),
                secret_token: SecretToken::from("secret".to_string()),
                cluster,
                user: None,
                user_data: None,
                subdomain: None,
            }),
        })
        .await;

    let response = handle.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let Some(backend_entry) = proxy.backend_entry(&backend_name) else {
        panic!("Backend entry not found");
    };
    assert_eq!(backend_entry.active_connections, 0);
    assert_eq!(backend_entry.had_recent_connection, true);
}

#[plane_test]
async fn proxy_marks_websocket_backend_as_active(env: TestEnvironment) {
    let server = WebSocketEchoServer::new().await;
    let backend_name = BackendName::new_random();

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("localhost:{}", port)).unwrap();
    let url = format!("ws://localhost:{port}/abc123/ws");

    let handle = tokio::spawn(connect_async(url));

    let route_info_request = proxy.recv_route_info_request().await;
    assert_eq!(
        route_info_request.token,
        BearerToken::from("abc123".to_string())
    );

    proxy
        .send_route_info_response(RouteInfoResponse {
            token: BearerToken::from("abc123".to_string()),
            route_info: Some(RouteInfo {
                backend_id: backend_name.clone(),
                address: BackendAddr(server.addr()),
                secret_token: SecretToken::from("secret".to_string()),
                cluster,
                user: None,
                user_data: None,
                subdomain: None,
            }),
        })
        .await;

    let (mut ws_stream, _) = handle.await.unwrap().unwrap();

    let Some(backend_entry) = proxy.backend_entry(&backend_name) else {
        panic!("Backend entry not found");
    };
    assert_eq!(backend_entry.active_connections, 1);
    assert_eq!(backend_entry.had_recent_connection, true);

    ws_stream.close(None).await.unwrap();
    drop(ws_stream);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let backend_entry = proxy.backend_entry(&backend_name).unwrap();
    assert_eq!(backend_entry.active_connections, 0);
    assert_eq!(backend_entry.had_recent_connection, true);
}
