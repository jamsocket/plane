use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use anyhow::{anyhow, Error};
#[cfg(feature = "bollard")]
use bollard::{container::LogOutput, container::Stats};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::{collections::HashMap, net::IpAddr, str::FromStr, time::Duration};

#[derive(PartialEq, Clone, Serialize, Deserialize, Debug)]
pub enum DockerCredentials {
    UsernamePassword { username: String, password: String },
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
            ..async_nats::jetstream::stream::Config::default()
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BackendStatsMessage {
    backend_id: BackendId,
    /// Fraction of maximum CPU.
    pub cpu_use_percent: f64,
    /// Fraction of maximum memory.
    pub mem_use_percent: f64,
}

impl TypedMessage for BackendStatsMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!("backend.{}.stats", self.backend_id.id())
    }
}

impl BackendStatsMessage {
    pub fn subscribe_subject(backend_id: &BackendId) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!("backend.{}.stats", backend_id.id()))
    }
}

impl BackendStatsMessage {
    #[cfg(feature = "bollard")]
    pub fn from_stats_messages(
        backend_id: &BackendId,
        prev_stats_message: &Stats,
        cur_stats_message: &Stats,
    ) -> Result<BackendStatsMessage, Error> {
        // Based on docs here: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats
        let mem_naive_usage = cur_stats_message
            .memory_stats
            .usage
            .ok_or_else(|| anyhow!("no memory stats.usage"))?;
        let mem_available = cur_stats_message
            .memory_stats
            .limit
            .ok_or_else(|| anyhow!("no memory stats.limit"))?;
        let mem_stats = cur_stats_message
            .memory_stats
            .stats
            .ok_or_else(|| anyhow!("no memory stats.stats"))?;
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
            .ok_or_else(|| anyhow!("no cpu_stats.system_cpu_usage"))?
            as f64)
            - (prev_cpu_stats
                .system_cpu_usage
                .ok_or_else(|| anyhow!("no cpu_stats.system_cpu_usage"))? as f64);
        // NOTE: we deviate from docker's formula here by not multiplying by num_cpus
        //       This is because what we actually want to know from this stat
        //       is what proportion of total cpu resource is consumed, and not knowing
        //       the top bound makes that impossible
        let cpu_use_percent = (cpu_delta as f64 / sys_cpu_delta) * 100.0;

        // TODO: implement disk stats from stream at
        //       https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerInspect

        Ok(BackendStatsMessage {
            backend_id: backend_id.clone(),
            cpu_use_percent,
            mem_use_percent,
        })
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DroneStatusMessage {
    pub drone_id: DroneId,
    pub cluster: ClusterName,
    pub drone_version: String,
}

impl TypedMessage for DroneStatusMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!("drone.{}.status", self.drone_id.id())
    }
}

impl JetStreamable for DroneStatusMessage {
    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().into(),
            subjects: vec!["drone.*.status".into()],
            max_messages_per_subject: 1,
            max_age: Duration::from_secs(5),
            ..async_nats::jetstream::stream::Config::default()
        }
    }

    fn stream_name() -> &'static str {
        "drone_status"
    }
}

impl DroneStatusMessage {
    pub fn subscribe_subject() -> SubscribeSubject<DroneStatusMessage> {
        SubscribeSubject::new("drone.*.status".to_string())
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
        "drone.register".to_string()
    }
}

impl DroneConnectRequest {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("drone.register".to_string())
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpawnRequest {
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
}

// eventually, this will be generic over executors
// currently only applies to docker
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct ResourceLimits {
    /// Period of cpu time, serializes as microseconds
    #[serde_as(as = "Option<DurationSeconds>")]
    pub cpu_period: Option<Duration>,

    /// Proportion of period used by container
    pub cpu_period_percent: Option<u8>,

    /// Total cpu time allocated to container    
    #[serde_as(as = "Option<DurationSeconds>")]
    pub cpu_time_limit: Option<Duration>,
}

impl TypedMessage for SpawnRequest {
    type Response = bool;

    fn subject(&self) -> String {
        format!("drone.{}.spawn", self.drone_id.id())
    }
}

impl SpawnRequest {
    pub fn subscribe_subject(drone_id: &DroneId) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!("drone.{}.spawn", drone_id.id()))
    }
}

/// A message telling a drone to terminate a backend.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TerminationRequest {
    pub backend_id: BackendId,
}

impl TypedMessage for TerminationRequest {
    type Response = bool;

    fn subject(&self) -> String {
        format!("backend.{}.terminate", self.backend_id.id())
    }
}

impl TerminationRequest {
    #[must_use]
    pub fn subscribe_subject() -> SubscribeSubject<TerminationRequest> {
        SubscribeSubject::new("backend.*.terminate".into())
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

    /// A timeout occurred becfore the container was ready.
    TimedOutBeforeReady,

    /// The container exited on its own initiative with a non-zero status.
    Failed,

    /// The container exited on its own initiative with a zero status.
    Exited,

    /// The container was terminated because all connections were closed.
    Swept,
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
        )
    }

    /// true if the state implies that the container is running.
    #[must_use]
    pub fn running(self) -> bool {
        matches!(self, BackendState::Starting | BackendState::Ready)
    }
}

/// An message representing a change in the state of a backend.
#[derive(Serialize, Deserialize, Debug)]
pub struct BackendStateMessage {
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
    pub fn new(state: BackendState, backend: BackendId) -> Self {
        BackendStateMessage {
            state,
            backend,
            time: Utc::now(),
        }
    }
}
