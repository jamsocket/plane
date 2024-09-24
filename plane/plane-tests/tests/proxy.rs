use std::{net::SocketAddr, str::FromStr};

use common::{proxy_mock::MockProxy, test_env::TestEnvironment};
use plane::{
    log_types::BackendAddr,
    names::{BackendName, Name},
    protocol::{RouteInfo, RouteInfoResponse},
    types::{BearerToken, ClusterName, SecretToken},
};
use plane_test_macro::plane_test;
use reqwest::StatusCode;

mod common;

#[plane_test]
async fn proxy_no_bearer_token(env: TestEnvironment) {
    let mut proxy = MockProxy::new().await;
    let url = format!("http://{}", proxy.addr());
    let handle = tokio::spawn(async { reqwest::get(url).await.expect("Failed to send request") });

    proxy.expect_no_route_info_request().await;

    let response = handle.await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[plane_test]
async fn proxy_bad_bearer_token(env: TestEnvironment) {
    let mut proxy = MockProxy::new().await;
    let url = format!("http://{}/abc123/", proxy.addr());
    let handle = tokio::spawn(async { reqwest::get(url).await.expect("Failed to send request") });

    let route_info_request = proxy.recv_route_info_request().await;
    assert_eq!(
        route_info_request.token,
        BearerToken::from("abc123".to_string())
    );

    proxy
        .send_route_info_response(RouteInfoResponse {
            token: BearerToken::from("abc123".to_string()),
            route_info: None,
        })
        .await;

    let response = handle.await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[plane_test]
async fn proxy_backend_unreachable(env: TestEnvironment) {
    let mut proxy = MockProxy::new().await;
    let url = format!("http://{}/abc123/", proxy.addr());
    let handle = tokio::spawn(async { reqwest::get(url).await.expect("Failed to send request") });

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
                address: BackendAddr(SocketAddr::from(([123, 234, 123, 234], 12345))),
                secret_token: SecretToken::from("secret".to_string()),
                cluster: ClusterName::from_str("test").unwrap(),
                user: None,
                user_data: None,
                subdomain: None,
            }),
        })
        .await;

    let response = handle.await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}
