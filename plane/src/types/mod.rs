use crate::{
    client::PlaneClient,
    names::{BackendName, DroneName},
    util::{random_prefixed_string, random_token},
};
pub use backend_state::{BackendState, BackendStatus, TerminationKind, TerminationReason};
use bollard::auth::DockerCredentials;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::HashMap, fmt::Display, str::FromStr};

pub mod backend_state;

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

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, Hash, valuable::Valuable)]
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

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, valuable::Valuable)]
pub enum PullPolicy {
    #[default]
    IfNotPresent,
    Always,
    Never,
}

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct DockerCpuPeriod(
    #[serde_as(as = "serde_with::DurationMicroSeconds<u64>")] std::time::Duration,
);

impl valuable::Valuable for DockerCpuPeriod {
    fn as_value(&self) -> valuable::Value {
        valuable::Value::U128(self.0.as_micros())
    }

    fn visit(&self, visit: &mut dyn valuable::Visit) {
        visit.visit_value(self.as_value())
    }
}

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

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct DockerCpuTimeLimit(
    #[serde_as(as = "serde_with::DurationSeconds<u64>")] pub std::time::Duration,
);

impl valuable::Valuable for DockerCpuTimeLimit {
    fn as_value(&self) -> valuable::Value {
        valuable::Value::U64(self.0.as_secs())
    }

    fn visit(&self, visit: &mut dyn valuable::Visit) {
        visit.visit_value(self.as_value())
    }
}

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, valuable::Valuable)]
pub struct ResourceLimits {
    /// Period of cpu time (de/serializes as microseconds)
    pub cpu_period: Option<DockerCpuPeriod>,

    /// Proportion of period used by container
    pub cpu_period_percent: Option<u8>,

    /// Total cpu time allocated to container
    pub cpu_time_limit: Option<DockerCpuTimeLimit>,

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

#[derive(Clone, Serialize, Deserialize, Debug, valuable::Valuable)]
#[serde(untagged)]
pub enum DockerRegistryAuth {
    UsernamePassword { username: String, password: String },
}

impl From<DockerRegistryAuth> for DockerCredentials {
    fn from(
        DockerRegistryAuth::UsernamePassword { username, password }: DockerRegistryAuth,
    ) -> Self {
        DockerCredentials {
            username: Some(username),
            password: Some(password),
            ..Default::default()
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, valuable::Valuable)]
pub struct ExecutorConfig {
    pub image: String,
    pub pull_policy: Option<PullPolicy>,
    pub credentials: Option<DockerRegistryAuth>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
}

impl ExecutorConfig {
    pub fn from_image_with_defaults<T: Into<String>>(image: T) -> Self {
        Self {
            image: image.into(),
            pull_policy: None,
            env: HashMap::default(),
            resource_limits: ResourceLimits::default(),
            credentials: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SpawnConfig {
    /// ID to assign to the new backend. Must be unique.
    /// This should only be used if you really need it, otherwise you can leave it blank
    /// and let Plane assign a unique ID automatically. This may be removed from
    /// future versions of Plane.
    pub id: Option<BackendName>,

    /// Cluster to spawn to. Uses the controller default if not provided.
    pub cluster: Option<ClusterName>,

    /// Config to use to spawn the backend process.
    pub executable: ExecutorConfig,

    /// If provided, the maximum amount of time the backend will be allowed to
    /// stay alive. Time counts from when the backend is scheduled.
    pub lifetime_limit_seconds: Option<i32>,

    /// If provided, the maximum amount of time the backend will be allowed to
    /// stay alive with no inbound connections to it.
    pub max_idle_seconds: Option<i32>,

    /// If true, the backend will have a single connection token associated with it at spawn
    /// time instead of dynamic tokens for each user.
    #[serde(default)]
    pub use_static_token: bool,
}

#[derive(
    Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq, Hash, valuable::Valuable,
)]
pub struct KeyConfig {
    /// If provided, and a running backend was created with the same key,
    /// namespace, and tag, we will connect to that backend instead
    /// of creating a new one.
    pub name: String,

    /// Namespace of the key. If not specified, the default namespace (empty string)
    /// is used.
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
    /// Configuration for the key to use.
    #[serde(default)]
    pub key: Option<KeyConfig>,

    /// Config to use if we need to create a new backend to connect to.
    pub spawn_config: Option<SpawnConfig>,

    /// Username or other identifier to associate with the generated connection URL.
    /// Passed to the backend through the X-Plane-User header.
    pub user: Option<String>,

    /// Arbitrary JSON object to pass along with each request to the backend.
    /// Passed to the backend through the X-Plane-Auth header.
    #[serde(default)]
    pub auth: Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, Hash, valuable::Valuable)]
pub struct BearerToken(String);

const STATIC_TOKEN_PREFIX: &str = "s.";

impl BearerToken {
    pub fn new_random_static() -> Self {
        Self(format!("{}{}", STATIC_TOKEN_PREFIX, random_token()))
    }

    pub fn is_static(&self) -> bool {
        self.0.starts_with(STATIC_TOKEN_PREFIX)
    }
}

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

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, valuable::Valuable)]
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

    pub status: BackendStatus,

    pub token: BearerToken,

    pub url: String,

    pub secret_token: Option<SecretToken>,

    pub status_url: String,

    /// The drone that spawned this backend, if the request resulted in a spawn.
    pub drone: Option<DroneName>,
}

impl ConnectResponse {
    pub fn new(
        backend_id: BackendName,
        cluster: &ClusterName,
        spawned: bool,
        status: BackendStatus,
        token: BearerToken,
        secret_token: Option<SecretToken>,
        client: &PlaneClient,
        drone: Option<DroneName>,
    ) -> Self {
        let url = if cluster.is_https() {
            format!("https://{}/{}/", cluster, token)
        } else {
            format!("http://{}/{}/", cluster, token)
        };

        let status_url = client.backend_status_url(&backend_id).to_string();

        Self {
            backend_id,
            spawned,
            status,
            token,
            url,
            secret_token,
            status_url,
            drone,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
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
