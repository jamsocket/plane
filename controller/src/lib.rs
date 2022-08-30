use anyhow::Result;
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

    // TODO: use jetstream
    let mut status_sub = nats
        .subscribe(DroneStatusMessage::subscribe_subject())
        .await?;

    loop {
        select! {
            status_msg = status_sub.next() => {
                let status_msg = status_msg.unwrap().unwrap();

                scheduler.update_status(Utc::now(), &status_msg.value);
            },

            spawn_request = spawn_request_sub.next() => {
                let spawn_request = spawn_request.unwrap().unwrap();

                let result = match scheduler.schedule() {
                    Ok(drone_id) => ScheduleResponse::Scheduled {
                        drone: drone_id
                    },
                    Err(_) => ScheduleResponse::NoDroneAvailable,
                };

                spawn_request.respond(&result).await?;
            }
        }
    }
}
