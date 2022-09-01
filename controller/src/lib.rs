use anyhow::Result;
use chrono::Utc;
use dis_spawner::{
    messages::agent::DroneStatusMessage,
    messages::scheduler::{ScheduleRequest, ScheduleResponse},
    nats::TypedNats,
};
use scheduler::Scheduler;
use tokio::select;
use tokio_stream::StreamExt;

mod scheduler;

pub async fn run_scheduler(nats: TypedNats) -> Result<()> {
    let scheduler = Scheduler::default();
    let mut spawn_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;

    let mut status_sub = Box::pin(
        nats.subscribe_jetstream(DroneStatusMessage::subscribe_subject()).await
    );

    loop {
        select! {
            status_msg = status_sub.next() => {
                let status_msg = status_msg.unwrap();

                scheduler.update_status(Utc::now(), &status_msg);
            },

            spawn_request = spawn_request_sub.next() => {
                let spawn_request = spawn_request.unwrap().unwrap();

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
            }
        }
    }
}
