use anyhow::Result;
use integration_test::integration_test;
use plane_controller::run_scheduler;
use plane_core::{
    messages::{
        agent::{DroneStatusMessage, SpawnRequest},
        scheduler::ScheduleResponse,
    },
    nats::TypedNats,
    types::{ClusterName, DroneId},
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, timeout},
    util::base_scheduler_request,
};
use plane_drone::agent::ready_loop;
use std::time::Duration;
use tokio::time::sleep;

const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct MockAgent {
    nats: TypedNats,
}

impl MockAgent {
    pub fn new(nats: TypedNats) -> Self {
        MockAgent { nats }
    }

    pub async fn schedule_drone(&self, drone_id: &DroneId) -> Result<ScheduleResponse> {
        // Subscribe to spawn requests for this drone, to ensure that the
        // scheduler sends them.
        let mut sub = self
            .nats
            .subscribe(SpawnRequest::subscribe_subject(drone_id))
            .await?;
        sleep(Duration::from_millis(100)).await;

        // Construct a scheduler request.
        let request = base_scheduler_request();

        // Publish scheduler request, but defer waiting for response.
        let mut response_handle = self.nats.split_request(&request).await?;

        // Expect a spawn request from the scheduler.
        let result = timeout(1_000, "Agent should receive spawn request.", sub.next())
            .await
            .unwrap();

        // Ensure that the SpawnRequest is as expected.
        assert_eq!(
            drone_id, &result.value.drone_id,
            "Scheduled drone did not match expectation."
        );

        // Acting as the agent, respond true to indicate successful spawn.
        result.respond(&true).await?;

        // Expect the scheduler to respond.
        let result = timeout(
            1_000,
            "Schedule request should be responded.",
            response_handle.response(),
        )
        .await?;

        Ok(result)
    }
}

#[integration_test]
async fn no_drone_available() -> Result<()> {
    let nats = Nats::new(false).await?;
    let nats_conn = nats.connection().await?;
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
    sleep(Duration::from_millis(100)).await;

    let request = base_scheduler_request();
    tracing::info!("Making spawn request.");
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
    let nats = Nats::new(false).await?;
    let nats_conn = nats.connection().await?;
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone());
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
    sleep(Duration::from_millis(100)).await;

    nats_conn
        .publish(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.to_string(),
        })
        .await?;

    let result = mock_agent.schedule_drone(&drone_id).await?;
    assert!(matches!(result, ScheduleResponse::Scheduled { drone, .. } if drone == drone_id));

    Ok(())
}

#[integration_test]
async fn restarting_nats() -> Result<()> {
    let mut nats = Nats::new(true).await?;
    let nats_conn = nats.connection().await?;
    let drone_id = DroneId::new_random();
    let mock_agent = MockAgent::new(nats_conn.clone());
    //let joined_futs = futures::future::join(run_scheduler(nats_conn.clone()), ready_loop());
    let _scheduler_guard = expect_to_stay_alive(run_scheduler(nats_conn.clone()));
    let cloned_did = drone_id.clone();
    let _readyloop_guard = expect_to_stay_alive(ready_loop(
        nats_conn.clone(),
        cloned_did.clone(),
        ClusterName::new("plane.test"),
    ));

    /*
    nats_conn
        .publish_jetstream(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.into(),
        })
        .await?;

    nats_conn
        .publish_jetstream(&DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: PLANE_VERSION.into(),
        })
        .await?;
    */
    /*
    tokio::spawn((|nats_conn: TypedNats, did: DroneId| async move {
        loop {
            nats_conn
                .publish_jetstream(&DroneStatusMessage {
                    cluster: ClusterName::new("plane.test"),
                    drone_id: did.clone(),
                    drone_version: PLANE_VERSION.to_string(),
                })
                .await
                .unwrap_or_else(|_| println!("timed out!"));
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })(nats_conn.clone(), drone_id.clone()));
    */

    //nats.container.pause().await?;
    //tokio::time::sleep(Duration::from_secs(30)).await;
    //nats.container.unpause().await?;

    nats.container.restart().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let result = mock_agent.schedule_drone(&drone_id.clone()).await?;
    assert!(matches!(result, ScheduleResponse::Scheduled { drone, .. } if drone == drone_id));

    Ok(())
}
