use crate::database::DroneDatabase;
use crate::proxy::{ProxyHttpsOptions, ProxyOptions};
use anyhow::{anyhow, Result};
use clap::Parser;
use keys::KeyCertPathPair;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::PathBuf;
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

mod database;
mod keys;
mod proxy;

#[derive(Parser)]
struct Opts {
    /// Path to sqlite3 database file to use for getting route information.
    ///
    /// This may be a file that does not exist. In this case, it will be created.
    #[clap(long)]
    db_path: String,

    /// The domain of the cluster that this drone serves.
    #[clap(long)]
    cluster_domain: String,

    /// Run the proxy server.
    #[clap(long)]
    proxy: bool,

    /// Port to listen for HTTP requests on.
    #[clap(long, default_value = "80")]
    http_port: u16,

    /// Port to listen for HTTPS requests on.
    #[clap(long, default_value = "443")]
    https_port: u16,

    /// Path to read private key from.
    #[clap(long)]
    https_private_key: Option<PathBuf>,

    /// Path to read certificate from.
    #[clap(long)]
    https_certificate: Option<PathBuf>,
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

async fn get_db(db_path: &str) -> Result<DroneDatabase> {
    let co = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(co).await?;
    migrate!("./migrations").run(&pool).await?;

    Ok(DroneDatabase::new(pool))
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;
    let opts = Opts::parse();
    let db = get_db(&opts.db_path).await?;

    if opts.proxy {
        let https_options = if let Some(private_key_path) = opts.https_private_key {
            let certificate_path = opts.https_certificate.ok_or_else(|| {
                anyhow!("--https-certificate must be provided if --https-private-key is.")
            })?;

            Some(ProxyHttpsOptions {
                port: opts.https_port,
                key_paths: KeyCertPathPair {
                    certificate_path,
                    private_key_path,
                },
            })
        } else {
            tracing::debug!("Skipping HTTPS because no --https-private-key was passed.");
            None
        };

        let serve_opts = ProxyOptions {
            db,
            http_port: opts.http_port,
            https_options,
            cluster: opts.cluster_domain,
        };

        proxy::serve(serve_opts).await?;
    }
    Ok(())
}
