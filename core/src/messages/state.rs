use super::agent::{BackendState, DroneState};
use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject, TypedMessage},
    types::{AsSubjectComponent, BackendId, ClusterName, DroneId, ResourceLock},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum WorldStateMessage {
    ClusterMessage {
        cluster: ClusterName,
        #[serde(flatten)]
        message: ClusterStateMessage,
    },
    Heartbeat {
        heartbeat: Option<Value>,
    },
}

impl WorldStateMessage {
    /// Whether this message can overwrite the previous message on the same subject.
    pub fn overwrite(&self) -> bool {
        #[allow(clippy::match_like_matches_macro)] // matches macro diminishes readability here.
        match self {
            WorldStateMessage::ClusterMessage { message, .. } => match message {
                ClusterStateMessage::DroneMessage(DroneMessage {
                    message: DroneMessageType::State { .. },
                    ..
                }) => false,
                ClusterStateMessage::BackendMessage(BackendMessage {
                    message: BackendMessageType::State { .. },
                    ..
                }) => false,
                _ => true,
            },
            WorldStateMessage::Heartbeat { .. } => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClusterStateMessage {
    LockMessage(ClusterLockMessage),
    DroneMessage(DroneMessage),
    BackendMessage(BackendMessage),
    AcmeMessage(AcmeDnsRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterLockMessage {
    pub lock: ResourceLock,
    pub uid: u64,
    pub message: ClusterLockMessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClusterLockMessageType {
    Announce,
    Revoke,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DroneMessage {
    pub drone: DroneId,

    #[serde(flatten)]
    pub message: DroneMessageType,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct DroneMeta {
    pub ip: IpAddr,
    pub version: String,
    pub git_hash: Option<String>,
}

impl DroneMeta {
    pub fn git_hash_short(&self) -> Option<String> {
        self.git_hash.as_ref().map(|hash| {
            if hash.len() >= 7 {
                hash[..7].to_string()
            } else {
                hash.to_string()
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DroneMessageType {
    Metadata(DroneMeta),
    State {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: chrono::DateTime<chrono::Utc>,
        state: DroneState,
    },
    KeepAlive {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: chrono::DateTime<chrono::Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendMessage {
    pub backend: BackendId,

    #[serde(flatten)]
    pub message: BackendMessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendLockAssignment {
    pub lock: ResourceLock,
    pub uid: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackendMessageType {
    Assignment {
        drone: DroneId,
        bearer_token: Option<String>,
        lock_assignment: Option<BackendLockAssignment>,
    },
    State {
        state: BackendState,
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcmeDnsRecord {
    pub value: String,
}

impl WorldStateMessage {
    pub fn subscribe_subject() -> SubscribeSubject<WorldStateMessage> {
        SubscribeSubject::new("state.>".into())
    }

    pub fn assignment_with_lock_subscribe_subject(
        cluster: &ClusterName,
        lock: &ResourceLock,
    ) -> SubscribeSubject<WorldStateMessage> {
        SubscribeSubject::<WorldStateMessage>::new(format!(
            "state.cluster.{}.backend.*.assignment.lock.{}",
            cluster.as_subject_component(),
            lock.as_subject_component()
        ))
    }
}

impl TypedMessage for WorldStateMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        match &self {
            WorldStateMessage::Heartbeat { .. } => "state.heartbeat".into(),
            WorldStateMessage::ClusterMessage { message, cluster } => match message {
                ClusterStateMessage::LockMessage(message) => match message.message {
                    ClusterLockMessageType::Announce => format!(
                        "state.cluster.{}.lock.{}.announce",
                        cluster.as_subject_component(),
                        message.lock.as_subject_component()
                    ),
                    ClusterLockMessageType::Revoke => format!(
                        "state.cluster.{}.lock.{}.revoke",
                        cluster.as_subject_component(),
                        message.lock.as_subject_component()
                    ),
                },
                ClusterStateMessage::DroneMessage(message) => match message.message {
                    DroneMessageType::Metadata { .. } => format!(
                        "state.cluster.{}.drone.{}.meta",
                        cluster.subject_name(),
                        message.drone
                    ),
                    DroneMessageType::State { state, .. } => format!(
                        "state.cluster.{}.drone.{}.status.{}",
                        cluster.subject_name(),
                        message.drone,
                        state,
                    ),
                    DroneMessageType::KeepAlive { .. } => format!(
                        "state.cluster.{}.drone.{}.keep_alive",
                        cluster.subject_name(),
                        message.drone
                    ),
                },
                ClusterStateMessage::BackendMessage(message) => match message.message {
                    BackendMessageType::Assignment {
                        lock_assignment: Some(BackendLockAssignment { ref lock, .. }),
                        ..
                    } => format!(
                        "state.cluster.{}.backend.{}.assignment.lock.{}",
                        cluster.subject_name(),
                        message.backend.as_subject_component(),
                        lock.as_subject_component()
                    ),
                    BackendMessageType::Assignment { .. } => format!(
                        "state.cluster.{}.backend.{}.assignment",
                        cluster.as_subject_component(),
                        message.backend.as_subject_component(),
                    ),
                    BackendMessageType::State { state, .. } => format!(
                        "state.cluster.{}.backend.{}.state.{}",
                        cluster.subject_name(),
                        message.backend.as_subject_component(),
                        state
                    ),
                },
                ClusterStateMessage::AcmeMessage(_) => {
                    format!("state.cluster.{}.acme", cluster.subject_name())
                }
            },
        }
    }
}

impl JetStreamable for WorldStateMessage {
    fn stream_name() -> &'static str {
        "plane_state"
    }

    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().to_string(),
            subjects: vec!["state.>".to_string()],
            max_messages_per_subject: 1,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::agent;
    use serde_json::json;

    #[test]
    fn test_deserialize_heartbeat_message() {
        let message = json!({
            "heartbeat": null,
        });

        let message: WorldStateMessage = serde_json::from_value(message).unwrap();
        assert_eq!(message, WorldStateMessage::Heartbeat { heartbeat: None });
    }

    #[test]
    fn test_deserialize_cluster_message() {
        // Ensures stability of cluster state message.

        let message = json!({
            "cluster": "cluster",
            "DroneMessage": {
                "State": {
                    "state": "Ready",
                    "timestamp": 1609459200000i64,
                },
                "drone": "drone",
            }
        });

        let message: WorldStateMessage = serde_json::from_value(message).unwrap();

        assert_eq!(
            message,
            WorldStateMessage::ClusterMessage {
                cluster: ClusterName::new("cluster"),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: DroneId::new("drone".into()),
                    message: DroneMessageType::State {
                        state: agent::DroneState::Ready,
                        timestamp: DateTime::parse_from_rfc3339("2021-01-01T00:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                    }
                })
            }
        );
    }
}
