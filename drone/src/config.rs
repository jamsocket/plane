use dis_spawner::{nats_connection::NatsConnectionSpec, types::DroneId};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};
use crate::{cert::acme::AcmeConfiguration, ip::IpSource, keys::KeyCertPathPair};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum NatsAuthentication {
    Token { token: String },
    User { username: String, password: String },
    // JWT
}

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize, Default)]
pub struct DockerConfig {
    pub runtime: Option<String>,
    #[serde(default)]
    pub connection: DockerConnection,
}

#[derive(Serialize, Deserialize)]
struct CertRefreshOptions {
    acme: AcmeConfiguration,
}

#[derive(Serialize, Deserialize)]
pub struct ProxyOptions {
    #[serde(default = "default_bind_address")]
    pub bind_ip: IpAddr,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
}

fn default_bind_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
}

fn default_https_port() -> u16 {
    443
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
    pub cert: Option<KeyCertPathPair>,

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
