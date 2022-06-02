use crate::nats::TypedSubject;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

/// A request from the drone to the DNS server telling it to set
/// a TXT record on the given domain with the given value.
#[derive(Serialize, Deserialize, Debug)]
pub struct SetAcmeDnsRecord {
    pub cluster: String,
    pub value: String,
}

impl TypedSubject for SetAcmeDnsRecord {
    type Response = ();

    fn subject(&self) -> String {
        "acme.set_dns_record".to_string()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpawnMetadata {
    pub cluster: String,
    pub service_name: String,
    pub image: String,
    pub backend_name: String,
    pub username: String,
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
    pub metadata: SpawnMetadata,
}

impl TypedSubject for SpawnMessage {
    type Response = bool;

    fn subject(&self) -> String {
        format!("drone.{}.spawn", self.drone_name)
    }
}
