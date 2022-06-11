use crate::database::DroneDatabase;
use agent::run_agent;
use anyhow::Result;
use cert::refresh_certificate;
use clap::Parser;
use cli::{DronePlan, Opts};
use futures::{future::select_all, Future};
use keys::KeyCertPathPair;
use proxy::serve;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::pin::Pin;
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

mod agent;
mod cert;
mod cli;
mod database;
mod keys;
mod messages;
mod nats;
mod proxy;
mod retry;
mod types;

fn init_tracing() -> Result<()> {
    let filter_layer =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info,sqlx=warn"))?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter_layer)
        .init();

    Ok(())
}

pub async fn get_db(db_path: &str) -> Result<DroneDatabase> {
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
    let plan = DronePlan::try_from(opts)?;

    match plan {
        DronePlan::RunService {
            proxy_options,
            agent_options,
        } => {
            let mut futs: Vec<Pin<Box<dyn Future<Output = Result<()>>>>> = vec![];

            if let Some(proxy_options) = proxy_options {
                futs.push(Box::pin(serve(proxy_options)));
            }

            if let Some(agent_options) = agent_options {
                futs.push(Box::pin(run_agent(agent_options)))
            }

            let (result, _, _) = select_all(futs.into_iter()).await;
            result?;
        }
        DronePlan::DoMigration { db_path } => {
            get_db(&db_path).await?;
        }
        DronePlan::DoCertificateRefresh {
            acme_server_url,
            cluster_domain,
            key_paths,
            nats_url,
        } => {
            refresh_certificate(&cluster_domain, &nats_url, &key_paths, &acme_server_url).await?;
        }
    }
    Ok(())
}
