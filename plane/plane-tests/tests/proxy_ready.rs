use common::{proxy_mock::MockProxy, test_env::TestEnvironment};
use plane_test_macro::plane_test;
use reqwest::StatusCode;

mod common;

#[plane_test]
async fn proxy_not_ready(env: TestEnvironment) {
    let proxy = MockProxy::new().await;
    let url = format!("http://{}/ready", proxy.addr());
    let response = reqwest::get(url).await.expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[plane_test]
async fn proxy_ready(env: TestEnvironment) {
    let proxy = MockProxy::new().await;
    proxy.set_ready(true);
    let url = format!("http://{}/ready", proxy.addr());
    let response = reqwest::get(url).await.expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::OK);
}

/// Tests that the proxy becomes ready when it connects to the controller.
/// It's surprisingly hard to test that the proxy becomes non-ready when the
/// controller shuts down, because we don't actually drop the connection
/// task (we rely on the process exiting to do that). This is kind of a bug,
/// at least for the purpose of testing.
#[plane_test]
async fn proxy_becomes_ready(env: TestEnvironment) {
    let controller = env.controller().await;
    let proxy = env.proxy(&controller).await.unwrap();

    let url = format!("http://127.0.0.1:{}/ready", proxy.port);
    let response = reqwest::get(&url).await.expect("Failed to send request");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Wait for the proxy to become ready.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let response = reqwest::get(&url).await.expect("Failed to send request");
    assert_eq!(response.status(), StatusCode::OK);
}
