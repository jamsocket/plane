use crate::{types::DroneId, nats::{Subject, SubscribeSubject}};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration, net::IpAddr};

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
pub struct SpawnMessage {
    /// The container image to run.
    pub image: String,

    /// The drone to spawn the backend on.
    pub drone_name: String,

    /// The name of the backend. This forms part of the hostname used to
    /// connect to the drone.
    pub backend_name: String,

    /// The timeout after which the drone is shut down if no connections are made.
    pub max_idle_time: Duration,

    /// Environment variables to pass in to the container.
    pub env: HashMap<String, String>,

    /// Metadata for the spawn. Typically added to log messages for debugging and observability.
    pub metadata: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum SpawnResult {

}

impl SpawnMessage {
    pub fn subject(drone_name: &str) -> Subject<SpawnMessage, SpawnResult> {
        Subject::new(format!("drone.{}.spawn", drone_name))
    }

    pub fn glob_subject() -> SubscribeSubject<SpawnMessage, SpawnResult> {
        SubscribeSubject::new("drone.*.spawn".to_string())
    }
}