use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject},
    types::{BackendId, ClusterName, DroneId},
};
#[allow(unused)] // Context is unused if bollard is not enabled.
use anyhow::{anyhow, Context, Error};
#[cfg(feature = "bollard")]
use bollard::container::{LogOutput, MemoryStatsStats, Stats};
use chrono::{DateTime, Utc};
use plane_core_nats_macros::{self, TypedMessage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    net::IpAddr,
    str::FromStr,
    time::Duration,
};

#[derive(PartialEq, Eq, Clone, Serialize, Deserialize, Debug)]
pub enum DockerCredentials {
    UsernamePassword { username: String, password: String },
    IdentityToken { identity_token: String },
}

#[cfg(feature = "bollard")]
impl From<&DockerCredentials> for bollard::auth::DockerCredentials {
    fn from(creds: &DockerCredentials) -> Self {
        match creds {
            DockerCredentials::UsernamePassword { username, password } => {
                bollard::auth::DockerCredentials {
                    username: Some(username.clone()),
                    password: Some(password.clone()),
                    ..bollard::auth::DockerCredentials::default()
                }
            }
            DockerCredentials::IdentityToken { identity_token } => {
                bollard::auth::DockerCredentials {
                    identitytoken: Some(identity_token.clone()),
                    ..bollard::auth::DockerCredentials::default()
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum DroneLogMessageKind {
    Stdout,
    Stderr,
    Meta,
}

#[derive(Serialize, Deserialize, Debug, TypedMessage)]
#[typed_message(subject = "backend.#backend_id.log")]
pub struct DroneLogMessage {
    pub backend_id: BackendId,
    pub kind: DroneLogMessageKind,
    pub text: String,
}

impl DroneLogMessage {
    #[cfg(feature = "bollard")]
    pub fn from_log_message(
        backend_id: &BackendId,
        log_message: &LogOutput,
    ) -> Option<DroneLogMessage> {
        match log_message {
            bollard::container::LogOutput::StdErr { message } => Some(DroneLogMessage {
                backend_id: backend_id.clone(),
                kind: DroneLogMessageKind::Stderr,
                text: std::str::from_utf8(message).ok()?.to_string(),
            }),
            bollard::container::LogOutput::StdOut { message } => Some(DroneLogMessage {
                backend_id: backend_id.clone(),
                kind: DroneLogMessageKind::Stdout,
                text: std::str::from_utf8(message).ok()?.to_string(),
            }),
            bollard::container::LogOutput::StdIn { message } => {
                tracing::warn!(?message, "Unexpected stdin message.");
                None
            }
            bollard::container::LogOutput::Console { message } => {
                tracing::warn!(?message, "Unexpected console message.");
                None
            }
        }
    }

    pub fn subscribe_subject(backend: &BackendId) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!("backend.{}.log", backend))
    }

    pub fn wildcard_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("backend.*.log".into())
    }
}

impl JetStreamable for DroneLogMessage {
    fn stream_name() -> &'static str {
        "backend_log"
    }

    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().into(),
            subjects: vec!["backend.*.log".into()],
            max_messages_per_subject: 500,
            ..async_nats::jetstream::stream::Config::default()
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, TypedMessage)]
#[typed_message(subject = "cluster.#cluster.backend.#backend_id.stats")]
pub struct BackendStatsMessage {
    pub cluster: ClusterName,
    pub drone_id: Option<DroneId>,
    pub backend_id: BackendId,
    /// Memory used by backend in bytes
    pub mem_used: u64,
    /// Total available memory for backend in bytes
    pub mem_available: u64,
    /// CPU cycles used by backend since last message
    pub cpu_used: u64,
    /// Total CPU cycles for system since last message
    pub sys_cpu: u64,
}

impl BackendStatsMessage {
    pub fn subscribe_subject(backend_id: &BackendId) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!("cluster.*.backend.{}.stats", backend_id.id()))
    }
}

impl BackendStatsMessage {
    #[cfg(feature = "bollard")]
    pub fn from_stats_messages(
        backend_id: &BackendId,
        drone_id: &DroneId,
        cluster: &ClusterName,
        prev_stats_message: &Stats,
        cur_stats_message: &Stats,
    ) -> Result<BackendStatsMessage, Error> {
        // Based on docs here: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats

        let mem_naive_usage = cur_stats_message
            .memory_stats
            .usage
            .context("no memory stats.usage")?;
        let mem_available = cur_stats_message
            .memory_stats
            .limit
            .context("no memory stats.limit")?;
        let mem_stats = cur_stats_message
            .memory_stats
            .stats
            .context("no memory stats.stats")?;
        let cache_mem = match mem_stats {
            MemoryStatsStats::V1(stats) => stats.cache,
            MemoryStatsStats::V2(stats) => stats.inactive_file,
        };
        let mem_used = mem_naive_usage - cache_mem;

        // REF: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats
        let cpu_stats = &cur_stats_message.cpu_stats;
        let prev_cpu_stats = &prev_stats_message.cpu_stats;
        // NOTE: total_usage gives clock cycles, this is monotonically increasing
        let cpu_used = cpu_stats.cpu_usage.total_usage - prev_cpu_stats.cpu_usage.total_usage;

        let prev_sys_cpu = prev_cpu_stats
            .system_cpu_usage
            .context("no cpu_stats.system_cpu_usage")?;
        let cur_sys_cpu = cpu_stats
            .system_cpu_usage
            .context("no cpu_stats.system_cpu_usage")?;
        let sys_cpu = cur_sys_cpu - prev_sys_cpu;

        // TODO: implement network I/O stats
        // TODO: implement disk stats from stream at
        //       https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerInspect

        Ok(BackendStatsMessage {
            backend_id: backend_id.clone(),
            drone_id: Some(drone_id.clone()),
            cluster: cluster.clone(),
            mem_used,
            mem_available,
            cpu_used,
            sys_cpu,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub enum DroneState {
    /// The drone is starting and is not ready to spawn backends.
    Starting,

    /// The drone is ready to spawn backends (subject to available capacity).
    Ready,

    /// The drone is running backends but is not accepting additional ones.
    Draining,

    /// The drone has finished draining and is ready to be shut down.
    Drained,

    /// The drone has shut down successfully.
    Stopped,

    /// The drone disappeared without a graceful shut-down and is assumed to be shut down.
    Lost,
}

impl Display for DroneState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DroneState::Starting => write!(f, "Starting"),
            DroneState::Ready => write!(f, "Ready"),
            DroneState::Draining => write!(f, "Draining"),
            DroneState::Drained => write!(f, "Drained"),
            DroneState::Stopped => write!(f, "Stopped"),
            DroneState::Lost => write!(f, "Lost"),
        }
    }
}

fn drone_state_ready() -> DroneState {
    DroneState::Ready
}

/// **DEPRECATED**. Will be removed in a future version of Plane.
#[derive(Serialize, Deserialize, Debug, TypedMessage)]
#[typed_message(subject = "cluster.#cluster.drone.#drone_id.status")]
pub struct DroneStatusMessage {
    pub drone_id: DroneId,
    pub cluster: ClusterName,
    pub drone_version: String,

    /// Indicates that a drone is ready to have backends scheduled to it.
    /// When a drone has been told to drain or is otherwise unable to have
    /// backends scheduled to it, this is set to false.
    #[serde(default = "default_ready")]
    pub ready: bool,

    #[serde(default = "drone_state_ready")]
    pub state: DroneState,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_backends: Option<u32>,
}

fn default_ready() -> bool {
    true
}

impl DroneStatusMessage {
    pub fn subscribe_subject() -> SubscribeSubject<DroneStatusMessage> {
        SubscribeSubject::new("cluster.*.drone.*.status".to_string())
    }
}

/// A message sent when a drone first connects to a controller.
#[derive(Serialize, Deserialize, Debug, Clone, TypedMessage)]
#[typed_message(subject = "cluster.#cluster.drone.register")]
pub struct DroneConnectRequest {
    /// The ID of the drone.
    pub drone_id: DroneId,

    /// The cluster the drone is requesting to join.
    pub cluster: ClusterName,

    /// The public-facing IP address of the drone.
    pub ip: IpAddr,
}

impl DroneConnectRequest {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.drone.register".to_string())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug, Default)]
pub enum DockerPullPolicy {
    #[default]
    IfNotPresent,
    Always,
    Never,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DockerExecutableConfig {
    /// The container image to run.
    pub image: String,

    /// Environment variables to pass in to the container.
    pub env: HashMap<String, String>,

    /// Credentials used to fetch the image.
    pub credentials: Option<DockerCredentials>,

    /// Resource limits
    #[serde(default = "ResourceLimits::default")]
    pub resource_limits: ResourceLimits,

    /// Pull policies, note: default is IfNotPresent
    #[serde(default = "DockerPullPolicy::default")]
    pub pull_policy: DockerPullPolicy,

    /// Port to serve on. If this is not set, Plane uses port 8080.
    pub port: Option<u16>,

    /// Volume mounts to add to the container.
    /// Schema: https://pkg.go.dev/github.com/docker/docker@v20.10.22+incompatible/api/types/mount#Mount
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_mounts: Vec<Value>,
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TypedMessage)]
#[typed_message(subject = "cluster.#cluster.drone.#drone_id.spawn", response = "bool")]
pub struct SpawnRequest {
    pub cluster: ClusterName,

    pub drone_id: DroneId,

    /// The timeout after which the drone is shut down if no connections are made.
    #[serde_as(as = "DurationSeconds")]
    pub max_idle_secs: Duration,

    /// The name of the backend. This forms part of the hostname used to
    /// connect to the drone.
    pub backend_id: BackendId,

    /// Metadata for the spawn. Typically added to log messages for debugging and observability.
    pub metadata: HashMap<String, String>,

    /// Configuration of executor (ie. image to run, executor being used etc)
    pub executable: DockerExecutableConfig,

    /// If set, the proxy will check for the given bearer token in requests (as
    /// a Bearer Authorization header, HTTP cookie, or query parameter) before
    /// allowing requests through.
    #[serde(default)]
    pub bearer_token: Option<String>,
}

// eventually, this will be generic over executors
// currently only applies to docker
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceLimits {
    /// Period of cpu time, serializes as microseconds
    #[serde_as(as = "Option<DurationSeconds>")]
    pub cpu_period: Option<Duration>,

    /// Proportion of period used by container
    pub cpu_period_percent: Option<u8>,

    /// Total cpu time allocated to container
    #[serde_as(as = "Option<DurationSeconds>")]
    pub cpu_time_limit: Option<Duration>,

    /// Maximum amount of memory container can use (in bytes)
    pub memory_limit_bytes: Option<i64>,

    /// Maximum disk space container can use (in bytes)
    pub disk_limit_bytes: Option<i64>,
}

impl SpawnRequest {
    pub fn subscribe_subject(cluster: &ClusterName, drone_id: &DroneId) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!(
            "cluster.{}.drone.{}.spawn",
            cluster.subject_name(),
            drone_id.id()
        ))
    }
}

/// A message telling a drone to terminate a backend.
#[derive(Serialize, Deserialize, Debug, Clone, TypedMessage)]
#[typed_message(
    subject = "cluster.#cluster_id.backend.#backend_id.terminate",
    response = "()"
)]
pub struct TerminationRequest {
    pub cluster_id: ClusterName,
    pub backend_id: BackendId,
}

impl TerminationRequest {
    #[must_use]
    pub fn subscribe_subject(cluster: &ClusterName) -> SubscribeSubject<TerminationRequest> {
        SubscribeSubject::new(format!(
            "cluster.{}.backend.*.terminate",
            cluster.subject_name()
        ))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BackendState {
    /// The backend has been created, and the image is being fetched.
    Loading,

    /// A failure occured while loading the image.
    ErrorLoading,

    /// The image has been fetched and is running, but is not yet listening
    /// on a port.
    Starting,

    /// A failure occured while starting the container.
    ErrorStarting,

    /// The container is listening on the expected port.
    Ready,

    /// A timeout occurred before the container was ready.
    TimedOutBeforeReady,

    /// The container exited on its own initiative with a non-zero status.
    Failed,

    /// The container exited on its own initiative with a zero status.
    Exited,

    /// The container was terminated because all connections were closed.
    Swept,

    /// The container was marked as lost.
    Lost,

    /// The container was terminated through the API.
    Terminated,
}

impl Display for BackendState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            BackendState::Loading => write!(f, "Loading"),
            BackendState::ErrorLoading => write!(f, "ErrorLoading"),
            BackendState::Starting => write!(f, "Starting"),
            BackendState::ErrorStarting => write!(f, "ErrorStarting"),
            BackendState::Ready => write!(f, "Ready"),
            BackendState::TimedOutBeforeReady => write!(f, "TimedOutBeforeReady"),
            BackendState::Failed => write!(f, "Failed"),
            BackendState::Exited => write!(f, "Exited"),
            BackendState::Swept => write!(f, "Swept"),
            BackendState::Lost => write!(f, "Lost"),
            BackendState::Terminated => write!(f, "Terminated"),
        }
    }
}

impl FromStr for BackendState {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Loading" => Ok(BackendState::Loading),
            "ErrorLoading" => Ok(BackendState::ErrorLoading),
            "Starting" => Ok(BackendState::Starting),
            "ErrorStarting" => Ok(BackendState::ErrorStarting),
            "Ready" => Ok(BackendState::Ready),
            "TimedOutBeforeReady" => Ok(BackendState::TimedOutBeforeReady),
            "Failed" => Ok(BackendState::Failed),
            "Exited" => Ok(BackendState::Exited),
            "Swept" => Ok(BackendState::Swept),
            "Lost" => Ok(BackendState::Lost),
            "Terminated" => Ok(BackendState::Terminated),
            _ => Err(anyhow!(
                "The string {:?} does not describe a valid state.",
                s
            )),
        }
    }
}

impl BackendState {
    /// true if the state is a final state of a backend that can not change.
    #[must_use]
    pub fn terminal(self) -> bool {
        matches!(
            self,
            BackendState::ErrorLoading
                | BackendState::ErrorStarting
                | BackendState::TimedOutBeforeReady
                | BackendState::Failed
                | BackendState::Exited
                | BackendState::Swept
                | BackendState::Lost
                | BackendState::Terminated
        )
    }

    /// true if the state implies that the container is running.
    #[must_use]
    pub fn running(self) -> bool {
        matches!(self, BackendState::Starting | BackendState::Ready)
    }
}

/// An message representing a change in the state of a backend.
/// **DEPRECATED** for drone-side use by UpdateBackendStateMessage.
/// Drones will send an UpdateBackendStateMessage and the controller
/// will publish a BackendStateMessage.
#[derive(Serialize, Deserialize, Debug, PartialEq, TypedMessage)]
#[typed_message(subject = "backend.#backend.status")]
pub struct BackendStateMessage {
    /// The cluster the backend belongs to.
    pub cluster: ClusterName,

    /// The new state.
    pub state: BackendState,

    /// The backend id.
    pub backend: BackendId,

    /// The time the state change was observed.
    pub time: DateTime<Utc>,
}

impl JetStreamable for BackendStateMessage {
    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().into(),
            subjects: vec!["backend.*.status".into()],
            ..async_nats::jetstream::stream::Config::default()
        }
    }

    fn stream_name() -> &'static str {
        "backend_status"
    }
}
impl BackendStateMessage {
    pub fn subscribe_subject(backend_id: &BackendId) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!("backend.{}.status", backend_id.id()))
    }

    pub fn wildcard_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("backend.*.status".into())
    }
}

impl BackendStateMessage {
    /// Construct a status message using the current time as its timestamp.
    #[must_use]
    pub fn new(state: BackendState, cluster: ClusterName, backend: BackendId) -> Self {
        BackendStateMessage {
            cluster,
            state,
            backend,
            time: Utc::now(),
        }
    }
}
