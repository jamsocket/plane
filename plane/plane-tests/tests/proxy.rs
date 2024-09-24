use common::{proxy_mock::MockProxy, test_env::TestEnvironment};
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
