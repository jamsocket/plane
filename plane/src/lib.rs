#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![doc = include_str!("../README.md")]

use serde::{Deserialize, Serialize};

pub mod admin;
pub mod cleanup;
pub mod client;
pub mod controller;
pub mod database;
pub mod dns;
pub mod drone;
pub mod heartbeat_consts;
pub mod init_tracing;
pub mod log_types;
pub mod names;
pub mod protocol;
pub mod proxy;
pub mod signals;
pub mod typed_socket;
pub mod types;
pub mod util;

/// The version of the plane binary from Cargo.toml.
pub const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The git hash of the plane binary (passed from build.rs)
pub const PLANE_GIT_HASH: &str = env!("GIT_HASH");

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlaneVersionInfo {
    pub version: String,
    pub git_hash: String,
}

pub fn plane_version_info() -> PlaneVersionInfo {
    PlaneVersionInfo {
        version: PLANE_VERSION.to_string(),
        git_hash: PLANE_GIT_HASH.to_string(),
    }
}
