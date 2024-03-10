use crate::{
    client::PlaneClient,
    drone::{docker::PlaneDocker, run_drone, DroneConfig},
    names::{DroneName, OrRandom},
    types::ClusterName,
    util::resolve_hostname,
};
use chrono::Duration;
use clap::Parser;
use std::{net::IpAddr, path::PathBuf};
use url::Url;

#[derive(Parser)]
pub struct DroneOpts {
    #[clap(long)]
    name: Option<DroneName>,

    #[clap(long)]
    controller_url: Url,

    #[clap(long)]
    cluster: ClusterName,

    /// IP address for this drone that proxies can connect to.
    #[clap(long, default_value = "127.0.0.1")]
    ip: String,

    /// Path to the database file. If omitted, an in-memory database will be used.
    #[clap(long)]
    db: Option<PathBuf>,

    #[clap(long)]
    docker_runtime: Option<String>,

    /// Optional log driver configuration, passed to Docker as the `LogConfig` field.
    #[clap(long)]
    log_config: Option<String>,

    /// Optional pool identifier. If present, will only schedule workloads with a matching `pool` tag on this drone.
    #[clap(long)]
    pool: Option<String>,

    /// Automatically prune stopped images.
    /// This prunes *all* unused container images, not just ones that Plane has loaded, so it is disabled by default.
    #[clap(long)]
    auto_prune_images: bool,

    /// Minimum age (in seconds) of backend containers to prune.
    /// By default, all stopped backends are pruned, but you can set this to a positive number of seconds to prune
    /// only backends that were created more than this many seconds ago.
    #[clap(long, default_value = "0")]
    auto_prune_containers_older_than_seconds: i32,
}

pub async fn drone_command(opts: DroneOpts) -> anyhow::Result<()> {
    let name = opts.name.or_random();
    tracing::info!(%name, "Starting drone");

    let client = PlaneClient::new(opts.controller_url);
    let docker = bollard::Docker::connect_with_local_defaults()?;

    let log_config = opts
        .log_config
        .map(|s| serde_json::from_str(&s))
        .transpose()?;

    let docker = PlaneDocker::new(docker, opts.docker_runtime, log_config).await?;

    let ip: IpAddr = resolve_hostname(&opts.ip)
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve hostname to IP address."))?;

    let cleanup_min_age = Duration::seconds(opts.auto_prune_containers_older_than_seconds as i64);

    let drone_config = DroneConfig {
        id: name.clone(),
        cluster: opts.cluster.clone(),
        ip,
        db_path: opts.db,
        pool: opts.pool.unwrap_or_default(),
        auto_prune: opts.auto_prune_images,
        cleanup_min_age,
    };

    run_drone(client, docker, &drone_config).await
}
