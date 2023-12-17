use common::test_env::TestEnvironment;
use plane2::database::{node::NodeConnectionStatusChangeNotification, subscribe::Subscription};
use plane_test_macro::plane_test;

mod common;

#[plane_test]
async fn controller_registers_drone(env: TestEnvironment) {
    let db = env.db().await;
    let controller = env.controller().await;
    let _drone = env.drone(&controller).await;

    let mut listener: Subscription<NodeConnectionStatusChangeNotification> = db.subscribe();

    let drone_status = listener.next().await.unwrap();

    let drone_status_list = db.node().list().await.unwrap();
    assert_eq!(drone_status_list.len(), 1);
    assert_eq!(drone_status_list[0].id, drone_status.payload.node_id);
}
