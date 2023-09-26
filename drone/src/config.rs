use crate::{cert::acme::AcmeConfiguration, ip::IpSource, keys::KeyCertPaths};
use plane_core::{nats_connection::NatsConnectionSpec, types::DroneId};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum NatsAuthentication {
    Token { token: String },
    User { username: String, password: String },
    // JWT
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum DockerConnection {
    Socket { socket: String },
    Http { http: String },
}

impl Default for DockerConnection {
    fn default() -> Self {
        DockerConnection::Socket {
            socket: "/var/run/docker.sock".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct DockerConfig {
    pub runtime: Option<String>,

    #[serde(default)]
    pub connection: DockerConnection,

    pub network: Option<String>,

    #[serde(default)]
    pub insecure_gpu: bool,

    /// If true, spawn requests will be allowed to pass volume mounts on to
    /// docker.
    #[serde(default)]
    pub allow_volume_mounts: bool,

    pub syslog: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CertRefreshOptions {
    acme: AcmeConfiguration,
}

#[derive(Serialize, Deserialize)]
pub struct ProxyOptions {
    #[serde(default = "default_bind_address")]
    pub bind_ip: IpAddr,
    pub https_port: Option<u16>,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    pub passthrough: Option<SocketAddr>,

    /// path routing and hostname routing are mutually exclusive.
    /// the _plane_backend prefix MUST be supplied if this option is true
    #[serde(default)]
    pub allow_path_routing: bool,
}

fn default_http_port() -> u16 {
    80
}

fn default_bind_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
}

#[derive(Serialize, Deserialize)]
pub struct AgentOptions {
    #[serde(default)]
    pub docker: DockerConfig,

    pub ip: IpSource,

    pub drone_id: Option<DroneId>,
}

#[derive(Serialize, Deserialize)]
pub struct DroneConfig {
    /// Unique string used to identify this drone. If not provided, a
    /// UUID is generated.
    pub drone_id: Option<DroneId>,

    /// Path to use for the sqlite3 database through which the agent and
    /// proxy share a persisted state.
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,

    /// The domain to which this drone belongs.
    pub cluster_domain: String,

    /// How to connect to NATS. Required by agent and certificate refresh.
    pub nats: Option<NatsConnectionSpec>,

    /// Paths to certificate and private key files. Required by proxy and
    /// certificate refresh.
    pub cert: Option<KeyCertPaths>,

    /// Settings for ACME certificate refresh. If not provided, certificates
    /// are not refreshed by this drone process.
    pub acme: Option<AcmeConfiguration>,

    /// Settings for the agent. If not provided, the agent does not run in
    /// this drone process.
    pub agent: Option<AgentOptions>,

    /// Settings for the proxy. If not provided, the proxy does not run in
    /// this drone process.
    pub proxy: Option<ProxyOptions>,
}

fn default_db_path() -> PathBuf {
    PathBuf::from("drone.sqlite3")
}
