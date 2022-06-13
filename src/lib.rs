use crate::database::DroneDatabase;
use anyhow::Result;
use keys::KeyCertPathPair;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

mod agent;
mod cert;
mod database;
pub mod drone;
mod keys;
mod messages;
mod nats;
mod proxy;
mod retry;
mod types;

pub fn init_tracing() -> Result<()> {
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
