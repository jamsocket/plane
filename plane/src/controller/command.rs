use super::ControllerConfig;
use anyhow::Result;
use clap::Parser;
use plane_client::{
    names::{ControllerName, Name},
    types::ClusterName,
};
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

    /// HTTP(S) URL to forward /ctrl/ requests to in order to check if they are authorized.
    /// The requests are stripped of their body and only include the original headers (and method),
    /// plus an additional `x-original-path` header with the original path
    /// (after stripping `/ctrl` and everything before it).
    #[clap(long)]
    forward_auth: Option<Url>,
}

impl ControllerOpts {
    pub fn into_config(self) -> Result<ControllerConfig> {
        let name = ControllerName::new_random();

        let controller_url = match self.controller_url {
            Some(url) => url,
            None => Url::parse(&format!("http://{}:{}", self.host, self.port))?,
        };

        let addr = (self.host, self.port).into();

        Ok(ControllerConfig {
            db_url: self.db,
            bind_addr: addr,
            id: name,
            controller_url,
            default_cluster: self.default_cluster,
            cleanup_min_age_days: self.cleanup_min_age_days,
            cleanup_batch_size: None,
            forward_auth: self.forward_auth,
        })
    }
}
