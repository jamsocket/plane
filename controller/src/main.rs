use anyhow::Result;
use clap::Parser;
use dis_spawner::nats_connection::NatsConnection;
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

    let nats_connection = NatsConnection::new(opts.nats_url)?;
    run_scheduler(nats_connection.connection().await?).await?;

    Ok(())
}
