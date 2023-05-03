use super::agent::{DockerExecutableConfig, ImageDownloadRequest, ResourceRequest, SpawnRequest};
use crate::{
    nats::{SubscribeSubject, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::{collections::HashMap, time::Duration};

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackendResource {
    pub backend_id: Option<BackendId>,

    /// The timeout after which the drone is shut down if no connections are made.
    #[serde_as(as = "DurationSeconds")]
    pub max_idle_secs: Duration,

    /// Configuration for docker run (image, creds, env vars etc.)
    pub executable: DockerExecutableConfig,

    #[serde(default)]
    pub require_bearer_token: bool,

    pub metadata: HashMap<String, String>,
}

impl BackendResource {
    pub fn schedule(&self, cluster: &ClusterName, drone_id: &DroneId) -> SpawnRequest {
        let backend_id = self
            .backend_id
            .clone()
            .unwrap_or_else(BackendId::new_random);

        let bearer_token = if self.require_bearer_token {
            let bearer_token: String = rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(30)
                .map(char::from)
                .collect();

            Some(bearer_token)
        } else {
            None
        };

        SpawnRequest {
            cluster: cluster.clone(),
            drone_id: drone_id.clone(),
            backend_id,
            max_idle_secs: self.max_idle_secs,
            metadata: self.metadata.clone(),
            executable: self.executable.clone(),
            bearer_token,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ImageResource {
    pub url: String,
}

impl ImageResource {
    pub fn schedule(&self, cluster: &ClusterName, drone_id: &DroneId) -> ImageDownloadRequest {
        ImageDownloadRequest {
            cluster: cluster.clone(),
            drone_id: drone_id.clone(),
            image_url: self.url.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Resource {
    Image(ImageResource),
    Backend(BackendResource),
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScheduleRequest {
    pub cluster: ClusterName,
    pub resource: Resource,
}

impl ScheduleRequest {
    pub fn schedule(&self, drone_id: &DroneId) -> ResourceRequest {
        match &self.resource {
            Resource::Image(s) => {
                ResourceRequest::ImageDownload(s.schedule(&self.cluster, drone_id))
            }
            Resource::Backend(s) => ResourceRequest::Spawn(s.schedule(&self.cluster, drone_id)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ScheduleResponse {
    //this is a quick hack, eventually scheduled should have a resource field instead of ScheduledX being enum variants
    ScheduledBackend {
        drone: DroneId,
        backend_id: BackendId,
        #[serde(skip_serializing_if = "Option::is_none")]
        bearer_token: Option<String>,
    },
    ScheduledImage {
        drone: DroneId,
        image: String,
    },
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

/// Message sent to a drone to tell it to start draining.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DrainDrone {
    pub drone: DroneId,
    pub cluster: ClusterName,
    pub drain: bool,
}

impl TypedMessage for DrainDrone {
    type Response = ();

    fn subject(&self) -> String {
        format!(
            "cluster.{}.drone.{}.drain",
            self.cluster.subject_name(),
            self.drone.id()
        )
    }
}

impl DrainDrone {
    pub fn subscribe_subject(drone: DroneId, cluster: ClusterName) -> SubscribeSubject<Self> {
        SubscribeSubject::new(format!(
            "cluster.{}.drone.{}.drain",
            cluster.subject_name(),
            drone.id()
        ))
    }
}
