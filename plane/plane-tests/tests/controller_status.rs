use common::test_env::TestEnvironment;
use common::timeout::WithTimeout;
use plane::database::{controller::ControllerHeartbeatNotification, subscribe::Subscription};
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
    let mut listener: Subscription<ControllerHeartbeatNotification> = db.subscribe();
    let mut controller = env.controller().await;

    let heartbeat = listener
        .next()
        .with_timeout(10)
        .await
        .expect("Did not receive first heartbeat message.")
        .unwrap();
    assert_eq!(&heartbeat.payload.id, controller.id());

    let online_controllers = db.controller().online_controllers().await.unwrap();

    assert_eq!(online_controllers.len(), 1);
    assert_eq!(&online_controllers[0].id, controller.id());

    // Stop the controller.
    controller.terminate().await;

    // Expect an "offline" heartbeat.
    let heartbeat = listener
        .next()
        .with_timeout(10)
        .await
        .expect("Did not receive second heartbeat message.")
        .unwrap();

    assert!(!heartbeat.payload.is_online);
    let online_controllers = db.controller().online_controllers().await.unwrap();
    assert_eq!(online_controllers.len(), 0);
}
