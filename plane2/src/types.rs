use crate::{
    client::PlaneClient,
    names::{BackendName, Name},
    util::random_prefixed_string,
};
use bollard::auth::DockerCredentials;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::HashMap, fmt::Display, net::SocketAddr, str::FromStr};

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Hash, Eq)]
pub struct NodeId(i32);

impl From<i32> for NodeId {
    fn from(i: i32) -> Self {
        Self(i)
    }
}

impl NodeId {
    pub fn as_i32(&self) -> i32 {
        self.0
    }
}

impl Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_i32())
    }
}

pub struct BackendKeyId(i32);

impl From<i32> for BackendKeyId {
    fn from(i: i32) -> Self {
        Self(i)
    }
}

impl BackendKeyId {
    pub fn as_i32(&self) -> i32 {
        self.0
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Hash, Eq)]
pub struct ClusterName(String);

impl ClusterName {
    pub fn is_https(&self) -> bool {
        let port = self.0.split_once(':').map(|x| x.1);
        port.is_none() || port == Some("443")
    }
}

impl Display for ClusterName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}

impl FromStr for ClusterName {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, ':');
        let host = parts.next().ok_or("missing hostname or ip")?;
        let port = parts.next();

        url::Host::parse(host).map_err(|_| "invalid hostname or ip")?;
        if let Some(port) = port {
            port.parse::<u16>().map_err(|_| "invalid port")?;
        }

        Ok(Self(s.to_string()))
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, PartialOrd)]
pub enum BackendStatus {
    /// The backend has been scheduled to a drone, but has not yet been acknowledged.
    /// This status is only assigned by the controller; the drone will never assign it by definition.
    Scheduled,

    /// The backend has been assigned to a drone, which is now responsible for loading its image.
    Loading,

    /// Telling Docker to start the container.
    Starting,

    /// Wait for the backend to be ready to accept connections.
    Waiting,

    /// The backend is listening for connections.
    Ready,

    /// The backend has been sent a SIGTERM, either because we sent it or the user did,
    /// and we are waiting for it to exit.
    /// Proxies should stop sending traffic to it, but we should not yet release the key.
    Terminating,

    /// The backend has exited or been swept.
    Terminated,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
pub enum TerminationKind {
    Soft,
    Hard,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct BackendState {
    pub status: BackendStatus,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SocketAddr>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination: Option<TerminationKind>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl BackendState {
    pub fn to_loading(&self) -> BackendState {
        BackendState {
            status: BackendStatus::Loading,
            ..self.clone()
        }
    }

    pub fn to_starting(&self) -> BackendState {
        BackendState {
            status: BackendStatus::Starting,
            ..self.clone()
        }
    }

    pub fn to_waiting(&self, address: SocketAddr) -> BackendState {
        BackendState {
            status: BackendStatus::Waiting,
            address: Some(address),
            ..self.clone()
        }
    }

    pub fn to_ready(&self) -> BackendState {
        BackendState {
            status: BackendStatus::Ready,
            ..self.clone()
        }
    }

    pub fn terminating(termination: TerminationKind) -> BackendState {
        BackendState {
            status: BackendStatus::Terminating,
            termination: Some(termination),
            address: None,
            exit_code: None,
        }
    }

    pub fn terminated(exit_code: Option<i32>) -> BackendState {
        BackendState {
            status: BackendStatus::Terminated,
            exit_code,
            ..Default::default()
        }
    }
}

impl Default for BackendState {
    fn default() -> Self {
        Self {
            status: BackendStatus::Scheduled,
            address: None,
            termination: None,
            exit_code: None,
        }
    }
}

impl TryFrom<String> for BackendStatus {
    type Error = serde_json::Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        serde_json::from_value(Value::String(s))
    }
}

impl Display for BackendStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let result = serde_json::to_value(self);
        match result {
            Ok(Value::String(v)) => write!(f, "{}", v),
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub enum PullPolicy {
    #[default]
    IfNotPresent,
    Always,
    Never,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DockerCpuPeriod(std::time::Duration);

impl Default for DockerCpuPeriod {
    fn default() -> Self {
        Self(std::time::Duration::from_millis(100))
    }
}

impl From<&DockerCpuPeriod> for std::time::Duration {
    fn from(value: &DockerCpuPeriod) -> Self {
        value.0
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceLimits {
    /// Period of cpu time
    pub cpu_period: Option<DockerCpuPeriod>,

    /// Proportion of period used by container
    pub cpu_period_percent: Option<u8>,

    /// Total cpu time allocated to container
    pub cpu_time_limit: Option<std::time::Duration>,

    /// Maximum amount of memory container can use (in bytes)
    pub memory_limit_bytes: Option<i64>,

    /// Maximum disk space container can use (in bytes)
    pub disk_limit_bytes: Option<i64>,
}

impl ResourceLimits {
    pub fn cpu_quota(&self) -> Option<std::time::Duration> {
        let Some(pc) = self.cpu_period_percent else {
            return None;
        };
        let cpu_period = self.cpu_period.clone().unwrap_or_default();

        let quota = cpu_period.0.mul_f64((pc as f64) / 100.0);
        Some(quota)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ExecutorConfig {
    pub image: String,
    pub pull_policy: PullPolicy,
    pub credentials: Option<DockerCredentials>,
    pub env: HashMap<String, String>,
    pub resource_limits: ResourceLimits,
}

impl ExecutorConfig {
    pub fn from_image_with_defaults<T: Into<String>>(image: T) -> Self {
        Self {
            image: image.into(),
            pull_policy: PullPolicy::default(),
            env: HashMap::default(),
            resource_limits: ResourceLimits::default(),
			credentials: None
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SpawnConfig {
    /// Config to use to spawn the backend process.
    pub executable: ExecutorConfig,

    /// If provided, the maximum amount of time the backend will be allowed to
    /// stay alive. Time counts from when the backend is scheduled.
    pub lifetime_limit_seconds: Option<i32>,

    /// If provided, the maximum amount of time the backend will be allowed to
    /// stay alive with no inbound connections to it.
    pub max_idle_seconds: Option<i32>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct KeyConfig {
    /// If provided, and a running backend was created with the same key,
    /// cluster, namespace, and tag, we will connect to that backend instead
    /// of creating a new one.
    pub name: String,

    /// Namespace of the key. If not specified, the default namespace (empty string)
    /// is used. Namespaces are scoped to a cluster, so two namespaces of the same name
    /// on different clusters are distinct.
    #[serde(default)]
    pub namespace: String,

    /// If we request a connection to a key and the backend for that key
    /// is running, we will only connect to it if the tag matches the tag
    /// of the connection request that created it.
    #[serde(default)]
    pub tag: String,
}

impl KeyConfig {
    pub fn new_random() -> Self {
        Self {
            name: random_prefixed_string("lk"),
            ..Default::default()
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct ConnectRequest {
    /// Config to use if we need to create a new backend to connect to.
    pub spawn_config: Option<SpawnConfig>,

    /// Configuration for the key to use.
    #[serde(default)]
    pub key: Option<KeyConfig>,

    /// Username or other identifier to associate with the generated connection URL.
    /// Passed to the backend through the X-Plane-User header.
    pub user: Option<String>,

    /// Arbitrary JSON object to pass along with each request to the backend.
    /// Passed to the backend through the X-Plane-Auth header.
    #[serde(default)]
    pub auth: Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, Hash)]
pub struct BearerToken(String);

impl From<String> for BearerToken {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl Display for BearerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct SecretToken(String);

impl From<String> for SecretToken {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl Display for SecretToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ConnectResponse {
    pub backend_id: BackendName,

    /// Whether the backend is a new one spawned due to the request.
    pub spawned: bool,

    pub token: BearerToken,

    pub url: String,

    pub secret_token: SecretToken,

    pub status_url: String,
}

impl ConnectResponse {
    pub fn new(
        backend_id: BackendName,
        cluster: &ClusterName,
        spawned: bool,
        token: BearerToken,
        secret_token: SecretToken,
        client: &PlaneClient,
    ) -> Self {
        let url = if cluster.is_https() {
            format!("https://{}/{}/", cluster, token)
        } else {
            format!("http://{}/{}/", cluster, token)
        };

        let status_url = client.backend_status_url(cluster, &backend_id).to_string();

        Self {
            backend_id,
            spawned,
            token,
            url,
            secret_token,
            status_url,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub enum NodeKind {
    Proxy,
    Drone,
    AcmeDnsServer,
}

impl Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let result = serde_json::to_value(self);
        match result {
            Ok(Value::String(v)) => write!(f, "{}", v),
            _ => unreachable!(),
        }
    }
}

impl TryFrom<String> for NodeKind {
    type Error = serde_json::Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        serde_json::from_value(Value::String(s))
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TimestampedBackendStatus {
    pub status: BackendStatus,

    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub time: DateTime<Utc>,
}
