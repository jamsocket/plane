use crate::{
    nats::{NoReply, Subject, SubscribeSubject},
    types::{BackendId, DroneId},
};
use bollard::auth::DockerCredentials;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::{collections::HashMap, net::IpAddr, str::FromStr, time::Duration};

#[derive(Serialize, Deserialize, Debug)]
pub enum DroneLogMessageKind {
    Stdout,
    Stderr,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DroneLogMessage {
    pub kind: DroneLogMessageKind,
    pub text: String,
}

impl DroneLogMessage {
    pub fn subject(backend_id: &BackendId) -> Subject<DroneLogMessage, NoReply> {
        Subject::new(format!("backend.{}.log", backend_id.id()))
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DroneStatusMessage {
    pub drone_id: DroneId,
    pub cluster: String,
    pub capacity: u32,
}

impl DroneStatusMessage {
    pub fn subject(drone_id: &DroneId) -> Subject<DroneStatusMessage, NoReply> {
        Subject::new(format!("drone.{}.status", drone_id.id()))
    }

    pub fn subscribe_subject() -> SubscribeSubject<DroneStatusMessage, bool> {
        SubscribeSubject::new("drone.*.status".to_string())
    }
}

/// A request from a drone to connect to the platform.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DroneConnectRequest {
    /// The cluster the drone is requesting to join.
    pub cluster: String,

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

impl DroneConnectRequest {
    pub fn subject() -> Subject<DroneConnectRequest, DroneConnectResponse> {
        Subject::new("drone.register".to_string())
    }
}

/// A message telling a drone to spawn a backend.
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpawnRequest {
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
}

impl SpawnRequest {
    pub fn subject(drone_id: DroneId) -> Subject<SpawnRequest, bool> {
        Subject::new(format!("drone.{}.spawn", drone_id.id()))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
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
    pub fn running(self) -> bool {
        matches!(
            self,
            BackendState::Starting | BackendState::Ready
        )
    }
}

/// An message representing a change in the state of a backend.
#[derive(Serialize, Deserialize, Debug)]
pub struct BackendStateMessage {
    /// The new state.
    pub state: BackendState,

    /// The time the state change was observed.
    pub time: DateTime<Utc>,
}

impl BackendStateMessage {
    /// Construct a status message using the current time as its timestamp.
    pub fn new(state: BackendState) -> Self {
        BackendStateMessage {
            state,
            time: Utc::now(),
        }
    }

    pub fn subject(backend_id: &BackendId) -> Subject<BackendStateMessage, NoReply> {
        Subject::new(format!("backend.{}.status", backend_id.id()))
    }

    pub fn subscribe_subject() -> SubscribeSubject<BackendStateMessage, NoReply> {
        SubscribeSubject::new("backend.*.status".to_string())
    }
}
