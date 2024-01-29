use common::test_env::TestEnvironment;
use plane_test_macro::plane_test;

mod common;

#[plane_test]
async fn controller_status_returns(env: TestEnvironment) {
    let controller = env.controller().await;
    let client = controller.client();
    client.status().await.unwrap();
}

#[plane_test]
async fn controller_registers_itself(env: TestEnvironment) {
    let db = env.db().await;
    let mut controller = env.controller().await;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let controllers = db.controller().online_controllers().await.unwrap();
    assert_eq!(1, controllers.len());
    assert_eq!(controller.id(), &controllers[0].id);

    // Stop the controller.
    controller.terminate().await;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let online_controllers = db.controller().online_controllers().await.unwrap();
    assert_eq!(online_controllers.len(), 0);
}
