use crate::{
    controller::run_controller,
    database::connect_and_migrate,
    names::{ControllerName, Name},
    types::ClusterName,
};
use anyhow::Context;
use clap::Parser;
use std::net::IpAddr;
use url::Url;

#[derive(Parser)]
pub struct ControllerOpts {
    #[clap(long)]
    db: String,

    #[clap(long, default_value = "8080")]
    port: u16,

    #[clap(long, default_value = "127.0.0.1")]
    host: IpAddr,

    #[clap(long)]
    controller_url: Option<Url>,

    #[clap(long)]
    default_cluster: Option<ClusterName>,

    #[clap(long)]
    cleanup_min_age_days: Option<i32>,
}

pub async fn controller_command(opts: ControllerOpts) -> anyhow::Result<()> {
    let name = ControllerName::new_random();

    let controller_url = match opts.controller_url {
        Some(url) => url,
        None => Url::parse(&format!("http://{}:{}", opts.host, opts.port))?,
    };

    tracing::info!(%name, "Starting controller. Attempting to connect to database...");
    let db = connect_and_migrate(&opts.db)
        .await
        .context("Failed to connect to database and run migrations.")?;
    tracing::info!("Connected to database.");

    let addr = (opts.host, opts.port).into();

    run_controller(
        db,
        addr,
        name,
        controller_url,
        opts.default_cluster,
        opts.cleanup_min_age_days,
    )
    .await
}
