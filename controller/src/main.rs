use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use dis_spawner::{
    messages::agent::DroneStatusMessage, messages::scheduler::ScheduleRequest,
    nats_connection::NatsConnection,
};
use scheduler::Scheduler;
use tokio::select;

mod scheduler;

#[derive(Parser)]
struct Opts {
    /// Hostname for connecting to NATS.
    #[clap(long, action)]
    pub nats_url: String,
}

async fn nats_listener(nats: &NatsConnection) -> Result<()> {
    let conn = nats.connection().await?;

    let scheduler = Scheduler::default();
    let mut spawn_request_sub = conn.subscribe(ScheduleRequest::subscribe_subject()).await?;

    // TODO: use jetstream
    let mut status_sub = conn
        .subscribe(DroneStatusMessage::subscribe_subject())
        .await?;

    loop {
        select! {
            status_msg = status_sub.next() => {
                let status_msg = status_msg.unwrap().unwrap();

                // TODO: time should come from message
                scheduler.update_status(Utc::now(), &status_msg.value);
            },

            spawn_request = spawn_request_sub.next() => {
                let spawn_request = spawn_request.unwrap().unwrap();

                let drone_id = scheduler.schedule();

                // TODO: respond with drone ID.
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    let nats_connection = NatsConnection::new(opts.nats_url)?;

    Ok(())
}
