pub mod record_stats;
pub mod utils;

use clap::{Parser, Subcommand};
use record_stats::{get_stats_recorder, Stats};
use std::error::Error;
use std::{thread, time};
use utils::{get_nats_sender, DroneStatsMessage, NatsSubjectComponent, Sender};

pub type ErrorObj = Box<dyn Error>;

const REPORTING_INTERVAL_MS: u64 = 1000;

#[derive(Parser, Debug)]
#[command(version, author, about, args_conflicts_with_subcommands = true)]
struct Opts {
    /// Enables test mode, writing to stdout instead of nats
    #[command(subcommand)]
    test: Option<Commands>,

    #[clap(flatten)]
    run: Option<RunArgs>,
}

#[derive(Debug, Parser)]
#[command(version, author, about)]
struct RunArgs {
    /// Set nats url
    #[clap(short, long)]
    nats_url: String,

    /// Plane cluster name
    #[clap(short, long)]
    cluster_name: NatsSubjectComponent,

    /// Drone id
    #[clap(short, long)]
    drone_id: NatsSubjectComponent,
}

#[derive(Subcommand, Debug)]
enum Commands {
    // writes to stdout instead of nats
    Test,
}

#[tokio::main]
async fn main() -> Result<(), ErrorObj> {
    let cli_opts = Opts::parse();
    let (cluster, drone, send): (_, _, Sender) = if let Some(Commands::Test) = cli_opts.test {
        let dummysend = Box::new(|msg: &str| -> Result<(), ErrorObj> {
            println!("{msg}");
            Ok(())
        });
        let cluster: String = "DUMMY_CLUSTER_NAME".into();
        let drone: String = "DUMMY_DRONE_ID".into();
        (cluster, drone, dummysend)
    } else {
        let cli_opts = RunArgs::parse();
        let nats_subject = format!(
            "cluster.{}.drone.{}.stats",
            cli_opts.cluster_name.0, cli_opts.drone_id.0
        );
        let send = get_nats_sender(&cli_opts.nats_url, &nats_subject).await?;
        (cli_opts.cluster_name.0, cli_opts.drone_id.0, send)
    };
    let mut record_stats = get_stats_recorder();

    let append_stats = |stats: Stats| DroneStatsMessage {
        cluster: cluster.clone(),
        drone: drone.clone(),
        stats,
    };

    loop {
        let stats = record_stats();
        let to_send = append_stats(stats);
        let stats_json = serde_json::to_string(&to_send)?;
        send(stats_json.as_str())?;
        thread::sleep(time::Duration::from_millis(REPORTING_INTERVAL_MS));
    }
}
