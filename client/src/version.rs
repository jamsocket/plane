use serde::{Deserialize, Serialize};

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

pub const SERVER_NAME: &str = concat!("Plane/", env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"));
