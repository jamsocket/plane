use anyhow::Result;
use clap::Parser;
use database::DroneDatabase;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tracing_subscriber::{EnvFilter, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt};
use std::path::PathBuf;

mod database;
mod proxy;

#[derive(Parser)]
struct Opts {
    /// Path to sqlite3 database file to use for getting route information.
    ///
    /// This may be a file that does not exist. In this case, it will be created.
    #[clap(long)]
    db_path: PathBuf,

    /// Port to listen for HTTP requests on.
    #[clap(long)]
    http_port: Option<u16>,
}

fn init_tracing() -> Result<()> {
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info,sqlx=warn"))?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter_layer)
        .init();
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;
    let opts = Opts::parse();

    let co = SqliteConnectOptions::new()
        .filename(opts.db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(co).await?;
    migrate!("./migrations").run(&pool).await?;

    let db = DroneDatabase::new(pool);

    if let Some(http_port) = opts.http_port {
        proxy::serve(db, http_port).await?;
    }

    Ok(())
}
