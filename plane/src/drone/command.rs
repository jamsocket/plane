use crate::{
    drone::{docker::PlaneDockerConfig, DroneConfig},
    names::{DroneName, OrRandom},
    types::ClusterName,
    util::resolve_hostname,
};
use anyhow::Result;
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

    /// Optional base directory under which backends are allowed to mount directories.
    #[clap(long)]
    mount_base: Option<PathBuf>,

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

impl DroneOpts {
    pub fn into_config(self) -> Result<DroneConfig> {
        let name = self.name.or_random();
        tracing::info!(%name, "Starting drone");

        let log_config = self
            .log_config
            .map(|s| serde_json::from_str(&s))
            .transpose()?;

        let docker_config = PlaneDockerConfig {
            runtime: self.docker_runtime,
            log_config,
            mount_base: self.mount_base,
        };

        let ip: IpAddr = resolve_hostname(&self.ip)
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve hostname to IP address."))?;

        let cleanup_min_age =
            Duration::try_seconds(self.auto_prune_containers_older_than_seconds as i64)
                .expect("valid duration");

        let drone_config = DroneConfig {
            controller_url: self.controller_url,
            id: name.clone(),
            cluster: self.cluster.clone(),
            ip,
            db_path: self.db,
            pool: self.pool.unwrap_or_default(),
            auto_prune: self.auto_prune_images,
            cleanup_min_age,
            docker_config,
        };

        Ok(drone_config)
    }
}
