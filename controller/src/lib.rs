use anyhow::{anyhow, Result};
use chrono::Utc;
use dis_spawner::{
    messages::agent::DroneStatusMessage,
    messages::scheduler::{ScheduleRequest, ScheduleResponse},
    nats::TypedNats,
};
use scheduler::Scheduler;
use tokio::select;

mod scheduler;

pub async fn run_scheduler(nats: TypedNats) -> Result<()> {
    let scheduler = Scheduler::default();
    let mut spawn_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    let mut status_sub = nats
        .subscribe_jetstream(DroneStatusMessage::subscribe_subject())
        .await?;
    tracing::info!("Subscribed to drone status messages.");

    loop {
        select! {
            status_msg = status_sub.next() => {
                tracing::info!(?status_msg, "Got drone status");
                if let Some(status_msg) = status_msg? {
                    scheduler.update_status(Utc::now(), &status_msg);
                } else {
                    return Err(anyhow!("status_sub.next() returned None."));
                }
            },

            spawn_request = spawn_request_sub.next() => {
                tracing::info!(?spawn_request, "Got spawn request");
                match spawn_request {
                    Ok(Some(spawn_request)) => {
                        let result = match scheduler.schedule() {
                            Ok(drone_id) => {
                                match nats.request(&spawn_request.value.schedule(drone_id)).await {
                                    Ok(false) | Err(_) => ScheduleResponse::NoDroneAvailable,
                                    Ok(true) => ScheduleResponse::Scheduled { drone: drone_id }
                                }
                            },
                            Err(_) => ScheduleResponse::NoDroneAvailable,
                        };

                        spawn_request.respond(&result).await?;
                    },
                    Ok(None) => return Err(anyhow!("spawn_request_sub.next() returned None.")),
                    Err(err) => tracing::warn!("spawn_request_sub.next() returned error: {:?}", err)
                }
            }
        }
    }
}
