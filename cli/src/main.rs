use anyhow::Result;
use async_nats::jetstream::consumer::DeliverPolicy;
use clap::{Parser, Subcommand};
use colored::Colorize;
use plane_core::{
    messages::{agent::DroneStatusMessage, dns::SetDnsRecord, scheduler::{ScheduleRequest, ScheduleResponse}},
    nats_connection::NatsConnectionSpec,
    types::ClusterName,
};
use std::{collections::HashMap, time::Duration};

#[derive(Parser)]
struct Opts {
    #[clap(long)]
    nats: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    ListDrones,
    ListDns,
    Spawn {
        cluster: String,
        image: String,
    },
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
        Command::Spawn { image, cluster } => {
            let result = nats
                .request(&ScheduleRequest {
                    backend_id: None,
                    cluster: ClusterName::new(&cluster),
                    image,
                    max_idle_secs: Duration::from_secs(30),
                    env: HashMap::new(),
                    metadata: HashMap::new(),
                    credentials: None,
                })
                .await?;

            match result {
                ScheduleResponse::Scheduled { drone, backend_id } => {
                    let url = format!("https://{}.{}", backend_id, cluster);

                    tracing::info!(
                        ?url,
                        ?drone,
                        ?backend_id,
                        "Backend scheduled."
                    );
                },
                ScheduleResponse::NoDroneAvailable => tracing::error!(
                    %cluster,
                    "Could not schedule backend because no drone was available for cluster."
                ),
            }
        }
        Command::ListDns => {
            let results = nats
                .get_all(
                    &SetDnsRecord::subscribe_subject(),
                    DeliverPolicy::LastPerSubject,
                )
                .await?;

            println!("Found {} DNS records:", results.len());

            for result in results {
                println!(
                    "{}.{}\t{}\t{}",
                    result.name.to_string().bright_magenta(),
                    result.cluster.to_string().bright_blue(),
                    result.kind.to_string().bright_cyan(),
                    result.value.to_string().bold()
                );
            }
        }
    }

    Ok(())
}
