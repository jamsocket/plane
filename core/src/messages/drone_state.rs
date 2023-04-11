use super::agent::{BackendState, DroneState};
use crate::{
    nats::{NoReply, SubscribeSubject, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;

fn drone_state_ready() -> DroneState {
    DroneState::Ready
}

fn default_ready() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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

/// A message sent when a drone first connects to a controller.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DroneConnectRequest {
    /// The ID of the drone.
    pub drone_id: DroneId,

    /// The version of Plane the drone is running.
    pub version: Option<String>,

    /// The git hash of the drone's version.
    pub git_hash: Option<String>,

    /// The cluster the drone is requesting to join.
    pub cluster: ClusterName,

    /// The public-facing IP address of the drone.
    pub ip: IpAddr,
}

impl TypedMessage for DroneConnectRequest {
    type Response = bool;

    fn subject(&self) -> String {
        format!("cluster.{}.drone.register", self.cluster.subject_name())
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum DroneStateUpdate {
    DroneStatusMessage(DroneStatusMessage),
    BackendStateMessage(UpdateBackendStateMessage),
    AcmeMessage(super::cert::SetAcmeDnsRecord),
    Connect(DroneConnectRequest),
}

impl TypedMessage for DroneStateUpdate {
    type Response = Option<Value>;

    fn subject(&self) -> String {
        match self {
            DroneStateUpdate::DroneStatusMessage(msg) => msg.subject(),
            DroneStateUpdate::BackendStateMessage(msg) => msg.subject(),
            DroneStateUpdate::Connect(msg) => msg.subject(),
            DroneStateUpdate::AcmeMessage(msg) => msg.subject(),
        }
    }
}

impl DroneStateUpdate {
    pub fn subscribe_subject_acme() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.set_acme_record".to_string())
    }

    pub fn subscribe_subject_connect() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.drone.register".to_string())
    }

    pub fn subscribe_subject_drone_status() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.drone.*.status".to_string())
    }

    pub fn subscribe_subject_backend_status() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.backend.*.status".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::DroneStateUpdate;
    use crate::nats::TypedMessage;

    #[test]
    fn test_serialize_drone_status_message() {
        let msg = super::DroneStatusMessage {
            drone_id: super::DroneId::new("test-drone".into()),
            cluster: super::ClusterName::new("test-cluster"),
            drone_version: "0.1.0".to_string(),
            ready: true,
            state: super::DroneState::Ready,
            running_backends: None,
        };

        let json = serde_json::to_value(&msg).unwrap();

        let msg2 = DroneStateUpdate::DroneStatusMessage(msg.clone());
        let json2 = serde_json::to_value(&msg2).unwrap();

        assert_eq!(json, json2);
        assert_eq!(msg.subject(), msg2.subject());
    }

    #[test]
    fn test_serialize_backend_state_message() {
        let msg = super::UpdateBackendStateMessage {
            cluster: super::ClusterName::new("test-cluster"),
            state: super::BackendState::Ready,
            backend: super::BackendId::new("test-backend".into()),
            time: chrono::Utc::now(),
            drone: super::DroneId::new("test-drone".into()),
        };

        let json = serde_json::to_value(&msg).unwrap();

        let msg2 = DroneStateUpdate::BackendStateMessage(msg.clone());
        let json2 = serde_json::to_value(&msg2).unwrap();

        assert_eq!(json, json2);
        assert_eq!(msg.subject(), msg2.subject());
    }

    #[test]
    fn test_serialize_drone_connect_message() {
        let msg = super::DroneConnectRequest {
            drone_id: super::DroneId::new("test-drone".into()),
            cluster: super::ClusterName::new("test-cluster"),
            ip: "12.12.12.12".parse().unwrap(),
            version: Some("0.1.0".to_string()),
            git_hash: Some("abcde".to_string()),
        };

        let json = serde_json::to_value(&msg).unwrap();

        let msg2 = DroneStateUpdate::Connect(msg.clone());
        let json2 = serde_json::to_value(&msg2).unwrap();

        assert_eq!(json, json2);
        assert_eq!(msg.subject(), msg2.subject());
    }
}
