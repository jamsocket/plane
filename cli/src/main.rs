use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use async_nats::jetstream::consumer::DeliverPolicy;
use clap::{Parser, Subcommand};
use colored::Colorize;
use dis_spawner::{
    messages::{agent::DroneStatusMessage, scheduler::ScheduleRequest},
    nats_connection::NatsConnectionSpec,
    types::{BackendId, ClusterName},
};
use uuid::Uuid;

#[derive(Parser)]
struct Opts {
    #[clap(long)]
    nats: Option<String>,

    #[clap(long, default_value = "spawner.test")]
    cluster: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    ListDrones,
    Spawn { image: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    tracing_subscriber::fmt().init();

    let nats = NatsConnectionSpec::from_url(opts.nats.as_deref().unwrap_or("nats://localhost"))?
        .connect()
        .await?;

    match opts.command {
        Command::ListDrones => {
            let drones = nats
                .get_all(
                    &DroneStatusMessage::subscribe_subject(),
                    DeliverPolicy::LastPerSubject,
                )
                .await?;

            println!("Found {} drones:", drones.len());

            for drone in drones {
                println!(
                    "{}\t{}",
                    drone.drone_id.to_string().bright_green(),
                    drone.cluster.to_string().bright_cyan()
                );
            }
        }
        Command::Spawn { image } => {
            let backend_id = Uuid::new_v4().to_string();
            let result = nats
                .request(&ScheduleRequest {
                    backend_id: BackendId::new(backend_id),
                    cluster: ClusterName::new(&opts.cluster),
                    image,
                    max_idle_secs: Duration::from_secs(30),
                    env: HashMap::new(),
                    metadata: HashMap::new(),
                    credentials: None,
                })
                .await?;

            println!("Spawn result: {:?}", result);
        }
    }

    Ok(())
}
