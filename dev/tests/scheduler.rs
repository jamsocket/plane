use anyhow::Result;
use dev::{resources::nats::Nats, timeout::expect_to_stay_alive};
use dis_spawner_controller::run_scheduler;
use integration_test::integration_test;

#[integration_test]
async fn single_drone_scheduler() -> Result<()> {
    let nats = Nats::new().await?;
    let nats_conn = nats.connection().await?;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn));

    Ok(())
}
