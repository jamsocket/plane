use anyhow::Result;
use async_nats::jetstream::consumer::DeliverPolicy;
use clap::{Parser, Subcommand};
use colored::Colorize;
use dis_spawner::{nats_connection::NatsConnectionSpec, messages::agent::DroneStatusMessage};

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
            let drones = nats.get_all(&DroneStatusMessage::subscribe_subject(), DeliverPolicy::LastPerSubject).await?;

            println!("Found {} drones:", drones.len());

            for drone in drones {
                println!("{}\t{}", drone.drone_id.to_string().bright_green(), drone.cluster.to_string().bright_cyan());
            }
        },
    }

    Ok(())
}
