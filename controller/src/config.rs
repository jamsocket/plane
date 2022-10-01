use std::net::{IpAddr, Ipv4Addr};

use dis_spawner::nats_connection::NatsConnectionSpec;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct SchedulerOptions {}

#[derive(Serialize, Deserialize)]
pub struct DnsOptions {
    #[serde(default="default_port")]
    pub port: u16,

    #[serde(default="default_bind_ip")]
    pub ip: IpAddr,
}

fn default_port() -> u16 {
    53
}

fn default_bind_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
}

#[derive(Serialize, Deserialize)]
pub struct ControllerConfig {
    /// How to connect to NATS.
    pub nats: NatsConnectionSpec,

    pub scheduler: Option<SchedulerOptions>,

    pub dns: Option<DnsOptions>,
}
