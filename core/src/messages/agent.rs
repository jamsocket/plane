use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use bollard::{auth::DockerCredentials, container::LogOutput, container::Stats};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::{DurationMicroSeconds, DurationSeconds};
use std::{collections::HashMap, net::IpAddr, str::FromStr, time::Duration};

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
    //just fractions of max for now, go from there
    backend_id: BackendId,
    pub cpu_use_percent: f64,
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
    pub fn from_stats_message(
        backend_id: &BackendId,
        stats_message: &Stats,
    ) -> Option<BackendStatsMessage> {
        // based on docs here: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats

        //memory
        let mem_naive_usage = stats_message.memory_stats.usage.unwrap_or_default();
        let mem_available = stats_message.memory_stats.limit.unwrap_or(u64::MAX);
        let mem_stats = stats_message.memory_stats.stats;
        let cache_mem = match mem_stats {
            Some(stats) => match stats {
                bollard::container::MemoryStatsStats::V1(stats) => stats.cache,
                bollard::container::MemoryStatsStats::V2(stats) => stats.inactive_file,
            },
            None => 0,
        };
        let used_memory = mem_naive_usage - cache_mem;
        let mem_use_percent = ((used_memory as f64) / (mem_available as f64)) * 100.0;

        //REF: https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerStats
        //cpu
        let cpu_stats = &stats_message.cpu_stats;
        let precpu_stats = &stats_message.precpu_stats;
        //NOTE: total_usage gives clock cycles, this is monotonically increasing
        let cpu_delta = cpu_stats.cpu_usage.total_usage - precpu_stats.cpu_usage.total_usage;
        if cpu_delta == 0 {
            return None;
        }
        let sys_cpu_delta = (cpu_stats.system_cpu_usage.unwrap_or_default() as f64)
            - (precpu_stats.system_cpu_usage.unwrap_or_default() as f64);
        let num_cpus = cpu_stats.online_cpus.unwrap_or_default();
        let cpu_use_percent = (cpu_delta as f64 / sys_cpu_delta) * (num_cpus as f64) * 100.0;
        //disk
        //TODO: stream https://docs.docker.com/engine/api/v1.41/#tag/Container/operation/ContainerInspect

        Some(BackendStatsMessage {
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
    pub capacity: u32,
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

/// A request from a drone to connect to the platform.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DroneConnectRequest {
    /// The cluster the drone is requesting to join.
    pub cluster: ClusterName,

    /// The public-facing IP address of the drone.
    pub ip: IpAddr,
}

/// A response from the platform to a drone's request to join.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DroneConnectResponse {
    /// The drone has joined the cluster and been given an ID.
    Success { drone_id: DroneId },

    /// The drone requested to join a cluster that does not exist.
    NoSuchCluster,
}

impl TypedMessage for DroneConnectRequest {
    type Response = DroneConnectResponse;

    fn subject(&self) -> String {
        "drone.register".to_string()
    }
}

impl DroneConnectRequest {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("drone.register".to_string())
    }
}

/// A message telling a drone to spawn a backend.
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpawnRequest {
    pub drone_id: DroneId,

    /// The container image to run.
    pub image: String,

    /// The name of the backend. This forms part of the hostname used to
    /// connect to the drone.
    pub backend_id: BackendId,

    /// The timeout after which the drone is shut down if no connections are made.
    #[serde_as(as = "DurationSeconds")]
    pub max_idle_secs: Duration,

    /// Environment variables to pass in to the container.
    pub env: HashMap<String, String>,

    /// Metadata for the spawn. Typically added to log messages for debugging and observability.
    pub metadata: HashMap<String, String>,

    /// Credentials used to fetch the image.
    pub credentials: Option<DockerCredentials>,

    /// Resource limits
    pub resource_limits: Option<ResourceLimits>,
}

// eventually, this will be generic over executors
// currently only applies to docker
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ResourceLimits {
    /// period of cpu time, serializes as microseconds
    #[serde_as(as = "DurationMicroSeconds")]
    pub cpu_period: Duration,

    /// proportion of period used by container
    pub cpu_period_percent: u8,

    /// total cpu time allocated to container    

    #[serde_as(as = "DurationSeconds")]
    pub cpu_time_limit: Duration,
}

impl Default for ResourceLimits {
    fn default() -> ResourceLimits {
        ResourceLimits {
            cpu_period: Duration::from_millis(100),
            cpu_period_percent: 100,
            cpu_time_limit: Duration::from_secs(30),
        }
    }
}

impl TypedMessage for SpawnRequest {
    type Response = bool;

    fn subject(&self) -> String {
        format!("drone.{}.spawn", self.drone_id.id())
    }
}

impl SpawnRequest {
    pub fn subscribe_subject(drone_id: DroneId) -> SubscribeSubject<Self> {
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
    type Err = anyhow::Error;

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
            _ => Err(anyhow::anyhow!(
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
