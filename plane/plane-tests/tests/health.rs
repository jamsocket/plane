use common::test_env::TestEnvironment;
use plane_test_macro::plane_test;

mod common;

#[plane_test]
async fn controller_health_check(env: TestEnvironment) {
    let controller = env.controller().await;
    let client = controller.client();
    let result = client.health_check().await;

    assert!(result.is_ok());
}
