use common::{auth_mock::MockAuthServer, test_env::TestEnvironment};
use plane_common::{PlaneClient, PlaneClientError};
use plane_test_macro::plane_test;
use reqwest::StatusCode;

mod common;

#[plane_test]
async fn forward_auth_rejects(env: TestEnvironment) {
    let mut mock_auth_server = MockAuthServer::new().await;
    let controller = env
        .controller_with_forward_auth(&mock_auth_server.url())
        .await;
    let client = controller.client();

    let task = tokio::spawn(async move { client.status().await });

    let mut req = mock_auth_server.expect().await.unwrap();
    req.reject();

    let res = task.await.unwrap();

    assert!(matches!(
        res,
        Err(PlaneClientError::UnexpectedStatus(StatusCode::UNAUTHORIZED))
    ));
}

#[plane_test]
async fn forward_auth_accepts(env: TestEnvironment) {
    let mut mock_auth_server = MockAuthServer::new().await;
    let controller = env
        .controller_with_forward_auth(&mock_auth_server.url())
        .await;
    let client = controller.client();

    let task = tokio::spawn(async move { client.status().await });

    let mut req = mock_auth_server.expect().await.unwrap();
    req.accept();

    let res = task.await.unwrap();

    assert!(res.is_ok());
}

#[plane_test]
async fn forward_auth_forwards_path(env: TestEnvironment) {
    let mut mock_auth_server = MockAuthServer::new().await;
    let controller = env
        .controller_with_forward_auth(&mock_auth_server.url())
        .await;
    let client = controller.client();

    tokio::spawn(async move {
        let _ = client.status().await;
    });

    let req = mock_auth_server.expect().await.unwrap();

    let http_req = req.request();

    assert_eq!(
        http_req.headers().get("x-original-path").unwrap(),
        "/status"
    );
}

#[plane_test]
async fn forward_auth_forwards_bearer_token(env: TestEnvironment) {
    let mut mock_auth_server = MockAuthServer::new().await;
    let controller = env
        .controller_with_forward_auth(&mock_auth_server.url())
        .await;

    // set username on url
    let mut controller_url = controller.url();
    controller_url.set_username("test").unwrap();

    let client = PlaneClient::new(controller_url);

    tokio::spawn(async move {
        let _ = client.status().await;
    });

    let req = mock_auth_server.expect().await.unwrap();

    let http_req = req.request();

    assert_eq!(
        http_req.headers().get("authorization").unwrap(),
        "Bearer test"
    );
}
