use common::{
    localhost_resolver::localhost_client, proxy_mock::MockProxy,
    simple_axum_server::SimpleAxumServer, test_env::TestEnvironment,
};
use plane_common::{
    log_types::BackendAddr,
    names::{BackendName, Name},
    protocol::{RouteInfo, RouteInfoResponse},
    types::{BearerToken, ClusterName, SecretToken},
};
use plane_test_macro::plane_test;
use reqwest::StatusCode;
use std::str::FromStr;

mod common;

#[plane_test]
async fn proxy_gone_request_has_cors_headers(env: TestEnvironment) {
    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let url = format!("http://plane.test:{port}/abc123/");
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
            route_info: None,
        })
        .await;

    let response = handle.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap()
            .to_str()
            .unwrap(),
        "*"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-methods")
            .unwrap()
            .to_str()
            .unwrap(),
        "*"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-headers")
            .unwrap()
            .to_str()
            .unwrap(),
        "*, Authorization"
    );
}

#[plane_test]
async fn proxy_valid_request_has_cors_headers(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;
    let mut proxy = MockProxy::new().await;
    let cluster = ClusterName::from_str(&format!("plane.test:{}", proxy.port())).unwrap();
    let port = proxy.port();
    let url = format!("http://plane.test:{port}/abc123/");
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
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap()
            .to_str()
            .unwrap(),
        "*"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-methods")
            .unwrap()
            .to_str()
            .unwrap(),
        "*"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-headers")
            .unwrap()
            .to_str()
            .unwrap(),
        "authorization, accept, content-type"
    );
}
