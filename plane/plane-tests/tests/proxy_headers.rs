use common::{
    localhost_resolver::localhost_client,
    proxy_mock::MockProxy,
    simple_axum_server::{RequestInfo, SimpleAxumServer},
    test_env::TestEnvironment,
};
use plane_common::{
    log_types::BackendAddr,
    names::{BackendName, Name},
    protocol::{RouteInfo, RouteInfoResponse},
    types::{BearerToken, ClusterName, SecretToken},
};
use plane_test_macro::plane_test;
use reqwest::StatusCode;
use serde_json::json;
use std::str::FromStr;

mod common;

#[plane_test]
async fn proxy_fake_verified_headers_are_stripped(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
    let url = format!("http://plane.test:{port}/abc123/");
    let client = localhost_client();

    let handle = tokio::spawn(
        client
            .get(url)
            .header("x-verified-blah", "foobar") // this header should be removed
            .header("this-header-is-ok", "blah") // this header should be preserved
            .send(),
    );

    let route_info_request = proxy.recv_route_info_request().await;
    assert_eq!(
        route_info_request.token,
        BearerToken::from("abc123".to_string())
    );

    proxy
        .send_route_info_response(RouteInfoResponse {
            token: BearerToken::from("abc123".to_string()),
            route_info: Some(RouteInfo {
                backend_id: BackendName::new_random(),
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
    let request_info: RequestInfo = response.json().await.unwrap();
    assert_eq!(request_info.path, "/");
    assert_eq!(request_info.method, "GET");

    assert_eq!(request_info.headers.get("x-verified-blah"), None);
    assert_eq!(
        request_info.headers.get("this-header-is-ok"),
        Some(&"blah".to_string())
    );
}

#[plane_test]
async fn proxy_plane_headers_are_set(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
    let url = format!("http://plane.test:{port}/abc123/a/b/c/");
    let client = localhost_client();

    let handle = tokio::spawn(client.get(url).send());

    let route_info_request = proxy.recv_route_info_request().await;
    assert_eq!(
        route_info_request.token,
        BearerToken::from("abc123".to_string())
    );

    proxy
        .send_route_info_response(RouteInfoResponse {
            token: BearerToken::from("abc123".to_string()),
            route_info: Some(RouteInfo {
                backend_id: BackendName::try_from("backend123".to_string()).unwrap(),
                address: BackendAddr(server.addr()),
                secret_token: SecretToken::from("secret987".to_string()),
                cluster,
                user: Some("auser123".to_string()),
                user_data: Some(json!({
                    "access": "readonly",
                    "email": "a@example.com",
                })),
                subdomain: None,
            }),
        })
        .await;

    let response = handle.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let request_info: RequestInfo = response.json().await.unwrap();
    assert_eq!(request_info.path, "/a/b/c/");
    assert_eq!(request_info.method, "GET");

    assert_eq!(
        request_info.headers.get("x-verified-username"),
        Some(&"auser123".to_string())
    );
    assert_eq!(
        request_info.headers.get("x-verified-user-data"),
        Some(&r#"{"access":"readonly","email":"a@example.com"}"#.to_string())
    );
    assert_eq!(
        request_info.headers.get("x-verified-secret"),
        Some(&"secret987".to_string())
    );
    assert_eq!(
        request_info.headers.get("x-verified-path"),
        Some(&"/abc123/a/b/c/".to_string())
    );
    assert_eq!(
        request_info.headers.get("x-verified-backend"),
        Some(&"backend123".to_string())
    );
}
