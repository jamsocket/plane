use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use anyhow::{anyhow, Error};
#[cfg(feature = "bollard")]
use bollard::{container::LogOutput, container::Stats};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::{collections::HashMap, net::IpAddr, str::FromStr, time::Duration};

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

#[derive(Serialize, Deserialize, Debug)]
pub enum DroneLogMessageKind {
    Stdout,
    Stderr,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DroneLogMessage {
    pub backend_id: BackendId,
    pub kind: DroneLogMessageKind,
    pub text: String,
}

impl TypedMessage for DroneLogMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!("backend.{}.log", self.backend_id.id())
    }
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackendStatsMessage {
    pub cluster: ClusterName,
    pub backend_id: BackendId,
    /// Fraction of maximum CPU.
    pub cpu_use_percent: f64,
    /// Fraction of maximum memory.
    pub mem_use_percent: f64,
}

impl TypedMessage for BackendStatsMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!(
            "cluster.{}.backend.{}.stats",
            self.cluster.subject_name(),
            self.backend_id.id()
        )
    }
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
        cluster: &ClusterName,
        prev_stats_message: &Stats,
        cur_stats_message: &Stats,
    ) -> Result<BackendStatsMessage, Error> {
        // Based on docs here: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats

        use anyhow::Context;
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
            bollard::container::MemoryStatsStats::V1(stats) => stats.cache,
            bollard::container::MemoryStatsStats::V2(stats) => stats.inactive_file,
        };
        let used_memory = mem_naive_usage - cache_mem;
        let mem_use_percent = ((used_memory as f64) / (mem_available as f64)) * 100.0;

        // REF: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats
        // cpu
        let cpu_stats = &cur_stats_message.cpu_stats;
        let prev_cpu_stats = &prev_stats_message.cpu_stats;
        // NOTE: total_usage gives clock cycles, this is monotonically increasing
        let cpu_delta = cpu_stats.cpu_usage.total_usage - prev_cpu_stats.cpu_usage.total_usage;
        let sys_cpu_delta = (cpu_stats
            .system_cpu_usage
            .context("no cpu_stats.system_cpu_usage")? as f64)
            - (prev_cpu_stats
                .system_cpu_usage
                .context("no cpu_stats.system_cpu_usage")? as f64);
        // NOTE: we deviate from docker's formula here by not multiplying by num_cpus
        //       This is because what we actually want to know from this stat
        //       is what proportion of total cpu resource is consumed, and not knowing
        //       the top bound makes that impossible
        let cpu_use_percent = (cpu_delta as f64 / sys_cpu_delta) * 100.0;

        // TODO: implement disk stats from stream at
        //       https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerInspect

        Ok(BackendStatsMessage {
            backend_id: backend_id.clone(),
            cluster: cluster.clone(),
            cpu_use_percent,
            mem_use_percent,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
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

fn drone_state_ready() -> DroneState {
    DroneState::Ready
}

/// **DEPRECATED**. Will be removed in a future version of Plane.
#[derive(Serialize, Deserialize, Debug)]
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

impl TypedMessage for DroneStatusMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!(
            "cluster.{}.drone.{}.status",
            self.cluster.subject_name(),
            self.drone_id.id()
        )
    }
}

impl DroneStatusMessage {
    pub fn subscribe_subject() -> SubscribeSubject<DroneStatusMessage> {
        SubscribeSubject::new("cluster.*.drone.*.status".to_string())
    }
}

/// A message sent when a drone first connects to a controller.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DroneConnectRequest {
    /// The ID of the drone.
    pub drone_id: DroneId,

    /// The cluster the drone is requesting to join.
    pub cluster: ClusterName,

    /// The public-facing IP address of the drone.
    pub ip: IpAddr,
}

impl TypedMessage for DroneConnectRequest {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!("cluster.{}.drone.register", self.cluster.subject_name())
    }
}

impl DroneConnectRequest {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.drone.register".to_string())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug)]
pub enum DockerPullPolicy {
    IfNotPresent,
    Always,
    Never,
}

impl Default for DockerPullPolicy {
    fn default() -> Self {
        DockerPullPolicy::IfNotPresent
    }
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
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
}

impl TypedMessage for SpawnRequest {
    type Response = bool;

    fn subject(&self) -> String {
        format!(
            "cluster.{}.drone.{}.spawn",
            self.cluster.subject_name(),
            self.drone_id.id()
        )
    }
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TerminationRequest {
    pub cluster_id: ClusterName,
    pub backend_id: BackendId,
}

impl TypedMessage for TerminationRequest {
    type Response = ();

    fn subject(&self) -> String {
        format!(
            "cluster.{}.backend.{}.terminate",
            self.cluster_id.subject_name(),
            self.backend_id.id()
        )
    }
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
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

impl ToString for BackendState {
    fn to_string(&self) -> String {
        match self {
            BackendState::Loading => "Loading".to_string(),
            BackendState::ErrorLoading => "ErrorLoading".to_string(),
            BackendState::Starting => "Starting".to_string(),
            BackendState::ErrorStarting => "ErrorStarting".to_string(),
            BackendState::Ready => "Ready".to_string(),
            BackendState::TimedOutBeforeReady => "TimedOutBeforeReady".to_string(),
            BackendState::Failed => "Failed".to_string(),
            BackendState::Exited => "Exited".to_string(),
            BackendState::Swept => "Swept".to_string(),
            BackendState::Lost => "Lost".to_string(),
            BackendState::Terminated => "Terminated".to_string(),
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateBackendStateMessage {
    /// The cluster the backend belongs to.
    pub cluster: ClusterName,

    /// The new state.
    pub state: BackendState,

    /// The backend id.
    pub backend: BackendId,

    /// The time the state change was observed.
    pub time: DateTime<Utc>,

    pub drone: DroneId,
}

impl TypedMessage for UpdateBackendStateMessage {
    type Response = ();

    fn subject(&self) -> String {
        format!(
            "cluster.{}.backend.{}.status",
            self.cluster.subject_name(),
            self.backend.id()
        )
    }
}

impl UpdateBackendStateMessage {
    #[must_use]
    pub fn subscribe_subject() -> SubscribeSubject<UpdateBackendStateMessage> {
        SubscribeSubject::new("cluster.*.backend.*.status".into())
    }

    #[must_use]
    pub fn backend_subject(
        cluster: &ClusterName,
        backend: &BackendId,
    ) -> SubscribeSubject<UpdateBackendStateMessage> {
        SubscribeSubject::new(format!(
            "cluster.{}.backend.{}.status",
            cluster.subject_name(),
            backend.id()
        ))
    }
}

/// An message representing a change in the state of a backend.
/// **DEPRECATED** for drone-side use by UpdateBackendStateMessage.
/// Drones will send an UpdateBackendStateMessage and the controller
/// will publish a BackendStateMessage.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
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

impl TypedMessage for BackendStateMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!("backend.{}.status", self.backend.id())
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
