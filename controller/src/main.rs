use anyhow::Result;
use clap::Parser;
use dis_spawner::nats_connection::NatsConnectionSpec;
use dis_spawner_controller::run_scheduler;

#[derive(Parser)]
struct Opts {
    /// Hostname for connecting to NATS.
    #[clap(long, action)]
    pub nats_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    let nats_connection = NatsConnectionSpec::from_url(&opts.nats_url)?;
    run_scheduler(nats_connection.connect().await?).await?;

    Ok(())
}
