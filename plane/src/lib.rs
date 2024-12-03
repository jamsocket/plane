#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![doc = include_str!("../README.md")]

use controller::ControllerConfig;
use dns::DnsConfig;
use drone::DroneConfig;
use plane_common::version::{PLANE_GIT_HASH, PLANE_VERSION};
use proxy::ProxyConfig;
use serde::{Deserialize, Serialize};

pub mod admin;
pub mod cleanup;
pub mod controller;
pub mod database;
pub mod dns;
pub mod drone;
pub mod heartbeat_consts;
pub mod init_tracing;
pub mod proxy;
pub mod signals;
pub mod typed_unix_socket;
pub mod util;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Plan {
    Controller(ControllerConfig),
    Dns(DnsConfig),
    Proxy(ProxyConfig),
    Drone(DroneConfig),
}
