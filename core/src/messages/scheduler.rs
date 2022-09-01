use crate::{
    nats::{SubscribeSubject, TypedMessage},
    types::{BackendId, DroneId},
};
use bollard::auth::DockerCredentials;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::{collections::HashMap, fmt::Display, time::Duration};

use super::agent::SpawnRequest;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ClusterId {
    hostname: String,
}

impl ClusterId {
    pub fn new(name: &str) -> Self {
        ClusterId {
            hostname: name.to_string(),
        }
    }

    pub fn subject_name(&self) -> String {
        self.hostname.replace('.', "_")
    }
}

impl Display for ClusterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.hostname.fmt(f)
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScheduleRequest {
    pub cluster: ClusterId,

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

impl ScheduleRequest {
    pub fn schedule(&self, drone_id: DroneId) -> SpawnRequest {
        SpawnRequest {
            drone_id,
            image: self.image.clone(),
            backend_id: self.backend_id.clone(),
            max_idle_secs: self.max_idle_secs,
            env: self.env.clone(),
            metadata: self.metadata.clone(),
            credentials: self.credentials.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ScheduleResponse {
    Scheduled { drone: DroneId },
    NoDroneAvailable,
}

impl TypedMessage for ScheduleRequest {
    type Response = ScheduleResponse;

    fn subject(&self) -> String {
        format!("cluster.{}.schedule", self.cluster.subject_name())
    }
}

impl ScheduleRequest {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.schedule".into())
    }
}
