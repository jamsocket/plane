use chrono::Utc;
use common::test_env::TestEnvironment;
use hyper::StatusCode;
use plane::{
    client::PlaneClientError,
    database::backend::BackendActionMessage,
    names::{DroneName, Name},
    protocol::{BackendAction, Heartbeat, MessageFromDrone, MessageToDrone},
    types::{ClusterName, ConnectRequest, ConnectResponse, ExecutorConfig, SpawnConfig},
};
use plane_test_macro::plane_test;
use std::time::Duration;

mod common;

/// Return a dummy connect request, which does not use a key.
fn connect_request(cluster: &ClusterName) -> ConnectRequest {
    ConnectRequest {
        spawn_config: Some(SpawnConfig {
            cluster: Some(cluster.clone()),
            executable: ExecutorConfig::from_image_with_defaults("alpine"),
            lifetime_limit_seconds: None,
            max_idle_seconds: None,
        }),
        ..Default::default()
    }
}

/// Tests that a connect fails when no drone is available.
#[plane_test]
async fn no_drone_available(env: TestEnvironment) {
    let controller = env.controller().await;

    let client = controller.client();
    let connect_request = connect_request(&env.cluster);

    let result = client.connect(&connect_request).await.unwrap_err();

    assert!(matches!(
        result,
        PlaneClientError::PlaneError(_, StatusCode::INTERNAL_SERVER_ERROR)
    ));
}

/// Tests that a connect succeeds when a drone is available.
#[plane_test]
async fn backend_action_resent_if_not_acked(env: TestEnvironment) {
    let drone_id = DroneName::new_random();
    let controller = env.controller().await;

    let backend_id = {
        let mut drone_connection = controller
            .client()
            .drone_connection(&env.cluster)
            .connect(&drone_id)
            .await
            .unwrap();

        tracing::info!("Sending initial heartbeat message (mocking the drone).");
        drone_connection
            .send(MessageFromDrone::Heartbeat(Heartbeat {
                local_time: Utc::now(),
            }))
            .await
            .unwrap();

        // Wait for the drone to be registered.
        tokio::time::sleep(Duration::from_millis(150)).await;

        tracing::info!("Issuing the connect request.");
        let client = controller.client();
        let connect_request = connect_request(&env.cluster);
        let result = client.connect(&connect_request).await.unwrap();

        let ConnectResponse {
            spawned: true,
            backend_id,
            ..
        } = result
        else {
            panic!("Unexpected response: {:?}", result);
        };

        let msg = drone_connection.recv().await.unwrap();
        let MessageToDrone::Action(BackendActionMessage {
            action: BackendAction::Spawn { .. },
            ..
        }) = msg
        else {
            panic!("Unexpected message: {:?}", msg);
        };

        drone_connection.close().await;

        backend_id
    };

    {
        // The message was not acked; a new connection should cause it to be repeated.
        let mut drone_connection = controller
            .client()
            .drone_connection(&env.cluster)
            .connect(&drone_id)
            .await
            .unwrap();

        let msg = drone_connection.recv().await.unwrap();

        let MessageToDrone::Action(BackendActionMessage {
            action: BackendAction::Spawn { .. },
            ..
        }) = msg
        else {
            panic!("Unexpected message: {:?}", msg);
        };

        tracing::info!("Acking the message.");
        let MessageToDrone::Action(BackendActionMessage { action_id, .. }) = msg else {
            panic!("Unexpected message: {:?}", msg);
        };

        drone_connection
            .send(MessageFromDrone::AckAction { action_id })
            .await
            .unwrap();

        // Drone connections should always be closed to prevent a warning, but this
        // one is important for the test because it ensures that we wait for the
        // ack to be sent before we close the stream.
        drone_connection.close().await;
    }

    // We don't currently have a way to make sure the ack has been processed.
    tokio::time::sleep(Duration::from_millis(50)).await;

    {
        // The message should not be repeated now.
        let mut drone_connection = controller
            .client()
            .drone_connection(&env.cluster)
            .connect(&drone_id)
            .await
            .unwrap();

        controller
            .client()
            .soft_terminate(&backend_id)
            .await
            .unwrap();

        let msg = drone_connection.recv().await.unwrap();

        let MessageToDrone::Action(BackendActionMessage {
            action: BackendAction::Terminate { .. },
            ..
        }) = msg
        else {
            panic!("Unexpected message: {:?}", msg);
        };

        drone_connection.close().await;
    }
}
