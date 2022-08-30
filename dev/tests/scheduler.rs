use anyhow::Result;
use dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, timeout},
    util::base_scheduler_request,
};
use dis_spawner::{messages::{scheduler::{ScheduleResponse, ClusterId}, agent::DroneStatusMessage}, types::DroneId};
use dis_spawner_controller::run_scheduler;
use integration_test::integration_test;
use std::time::Duration;
use tokio::time::sleep;

#[integration_test]
async fn no_drone_available() -> Result<()> {
    let nats = Nats::new().await?;
    let nats_conn = nats.connection().await?;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
    sleep(Duration::from_millis(100)).await;

    let request = base_scheduler_request();
    let result = timeout(
        1_000,
        "Schedule request should be responded.",
        nats_conn.request(&request),
    )
    .await?;

    assert_eq!(ScheduleResponse::NoDroneAvailable, result);

    Ok(())
}

#[integration_test]
async fn one_drone_available() -> Result<()> {
    let nats = Nats::new().await?;
    let nats_conn = nats.connection().await?;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
    sleep(Duration::from_millis(100)).await;

    nats_conn.publish(&DroneStatusMessage {
        capacity: 100,
        cluster: ClusterId::new("spawner.test"),
        drone_id: DroneId::new(8),
    }).await?;
    sleep(Duration::from_millis(100)).await;

    let request = base_scheduler_request();
    let result = timeout(
        1_000,
        "Schedule request should be responded.",
        nats_conn.request(&request),
    )
    .await?;

    assert_eq!(ScheduleResponse::Scheduled { drone: DroneId::new(8) }, result);

    Ok(())
}
