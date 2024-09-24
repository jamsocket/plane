use common::{
    localhost_resolver::localhost_client,
    proxy_mock::MockProxy,
    simple_axum_server::{RequestInfo, SimpleAxumServer},
    test_env::TestEnvironment,
};
use plane::{
    log_types::BackendAddr,
    names::{BackendName, Name},
    protocol::{RouteInfo, RouteInfoResponse},
    types::{BearerToken, ClusterName, SecretToken, Subdomain},
};
use plane_test_macro::plane_test;
use reqwest::StatusCode;
use std::{net::SocketAddr, str::FromStr};
use tokio::net::TcpListener;

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

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[plane_test]
async fn proxy_backend_unreachable(env: TestEnvironment) {
    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
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
                address: BackendAddr(SocketAddr::from(([123, 234, 123, 234], 12345))),
                secret_token: SecretToken::from("secret".to_string()),
                cluster,
                user: None,
                user_data: None,
                subdomain: None,
            }),
        })
        .await;

    let response = handle.await.unwrap().unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[plane_test]
async fn proxy_backend_timeout(env: TestEnvironment) {
    // We will start a listener, but never respond on it, to simulate a timeout.
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
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
                address: BackendAddr(addr),
                secret_token: SecretToken::from("secret".to_string()),
                cluster,
                user: None,
                user_data: None,
                subdomain: None,
            }),
        })
        .await;

    let response = handle.await.unwrap().unwrap();

    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
}

#[plane_test]
async fn proxy_backend_accepts(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
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
    let request_info: RequestInfo = response.json().await.unwrap();
    assert_eq!(request_info.path, "/");
    assert_eq!(request_info.method, "GET");
}

#[plane_test]
async fn proxy_expected_subdomain_not_present(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
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
                subdomain: Some(Subdomain::from_str("missing-subdomain").unwrap()),
            }),
        })
        .await;

    let response = handle.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[plane_test]
async fn proxy_expected_subdomain_is_present(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
    let url = format!("http://mysubdomain.plane.test:{port}/abc123/");
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
                subdomain: Some(Subdomain::from_str("mysubdomain").unwrap()),
            }),
        })
        .await;

    let response = handle.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[plane_test]
async fn proxy_backend_passes_forwarded_headers(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
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
    let request_info: RequestInfo = response.json().await.unwrap();
    let headers = request_info.headers;
    assert_eq!(headers.get("x-forwarded-for").unwrap(), "127.0.0.1");
    assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
}

#[plane_test]
async fn proxy_returns_backend_id_in_header(env: TestEnvironment) {
    let server = SimpleAxumServer::new().await;

    let mut proxy = MockProxy::new().await;
    let port = proxy.port();
    let cluster = ClusterName::from_str(&format!("plane.test:{}", port)).unwrap();
    let url = format!("http://plane.test:{port}/abc123/");
    let client = localhost_client();
    let handle = tokio::spawn(client.get(url).send());

    let backend_id = BackendName::new_random();
    let route_info_request = proxy.recv_route_info_request().await;
    assert_eq!(
        route_info_request.token,
        BearerToken::from("abc123".to_string())
    );

    proxy
        .send_route_info_response(RouteInfoResponse {
            token: BearerToken::from("abc123".to_string()),
            route_info: Some(RouteInfo {
                backend_id: backend_id.clone(),
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
    let headers = response.headers();
    assert_eq!(
        headers.get("x-plane-backend-id").unwrap().to_str().unwrap(),
        &backend_id.to_string()
    );
}
