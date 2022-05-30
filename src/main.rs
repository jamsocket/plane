use anyhow::Result;
use clap::Parser;
use database::DroneDatabase;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::PathBuf;
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

mod database;
mod proxy;

#[derive(Parser)]
struct Opts {
    /// Path to sqlite3 database file to use for getting route information.
    ///
    /// This may be a file that does not exist. In this case, it will be created.
    #[clap(long)]
    db_path: PathBuf,

    /// Run the proxy server.
    #[clap(long)]
    proxy: bool,

    /// Port to listen for HTTP requests on.
    #[clap(long, default_value = "80")]
    http_port: u16,

    /// Path to read private key from.
    #[clap(long)]
    https_private_key: Option<String>,

    /// Path to read certificate from.
    #[clap(long)]
    https_certificate: Option<String>,
}

fn init_tracing() -> Result<()> {
    let filter_layer =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info,sqlx=warn"))?;

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

    if opts.proxy {
        proxy::serve(db, opts.http_port).await?;
    }

    Ok(())
}
