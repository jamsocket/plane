use crate::{nats::{Subject, NoReply}, types::{DroneId, BackendId}};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr, time::Duration};

/// A request from a drone to connect to the platform.
#[derive(Serialize, Deserialize, Debug)]
pub struct DroneConnectRequest {
    /// The cluster the drone is requesting to join.
    pub cluster: String,

    /// The public-facing IP address of the drone.
    pub ip: IpAddr,
}

/// A response from the platform to a drone's request to join.
#[derive(Serialize, Deserialize, Debug)]
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
#[derive(Serialize, Deserialize, Debug)]
pub struct SpawnRequest {
    /// The container image to run.
    pub image: String,

    /// The name of the backend. This forms part of the hostname used to
    /// connect to the drone.
    pub backend_id: BackendId,

    /// The timeout after which the drone is shut down if no connections are made.
    pub max_idle_time: Duration,

    /// Environment variables to pass in to the container.
    pub env: HashMap<String, String>,

    /// Metadata for the spawn. Typically added to log messages for debugging and observability.
    pub metadata: HashMap<String, String>,
}

impl SpawnRequest {
    pub fn subject(drone_id: DroneId) -> Subject<SpawnRequest, bool> {
        Subject::new(format!("drone.{}.spawn", drone_id.id()))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
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

    pub fn subject(backend_id: BackendId) -> Subject<BackendStateMessage, NoReply> {
        Subject::new(format!("backend.{}.status", backend_id.id()))
    }
}
