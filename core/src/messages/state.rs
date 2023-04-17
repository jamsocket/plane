use super::agent::{BackendState, DroneState};
use crate::{
    nats::{JetStreamable, NoReply, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldStateMessage {
    pub cluster: ClusterName,

    #[serde(flatten)]
    pub message: ClusterStateMessage,
}

impl WorldStateMessage {
    /// Whether this message can overwrite the previous message on the same subject.
    pub fn overwrite(&self) -> bool {
        !matches!(
            self.message,
            ClusterStateMessage::DroneMessage(DroneMessage {
                message: DroneMessageType::State { .. },
                ..
            }) | ClusterStateMessage::BackendMessage(BackendMessage {
                message: BackendMessageType::State { .. },
                ..
            })
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClusterStateMessage {
    DroneMessage(DroneMessage),
    BackendMessage(BackendMessage),
    AcmeMessage(AcmeDnsRecord),
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
        self.git_hash.as_ref().map(|hash| hash[..7].to_string())
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
pub enum BackendMessageType {
    Assignment {
        drone: DroneId,
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

impl TypedMessage for WorldStateMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        match &self.message {
            ClusterStateMessage::DroneMessage(message) => match message.message {
                DroneMessageType::Metadata { .. } => {
                    format!(
                        "state.cluster.{}.drone.{}.meta",
                        self.cluster.subject_name(),
                        message.drone
                    )
                }
                DroneMessageType::State { state, .. } => {
                    format!(
                        "state.cluster.{}.drone.{}.status.{}",
                        self.cluster.subject_name(),
                        message.drone,
                        state,
                    )
                }
                DroneMessageType::KeepAlive { .. } => {
                    format!(
                        "state.cluster.{}.drone.{}.keep_alive",
                        self.cluster.subject_name(),
                        message.drone
                    )
                }
            },
            ClusterStateMessage::BackendMessage(message) => match message.message {
                BackendMessageType::Assignment { .. } => {
                    format!(
                        "state.cluster.{}.backend.{}",
                        self.cluster.subject_name(),
                        message.backend
                    )
                }
                BackendMessageType::State { state: status, .. } => {
                    format!(
                        "state.cluster.{}.backend.{}.state.{}",
                        self.cluster.subject_name(),
                        message.backend,
                        status
                    )
                }
            },
            ClusterStateMessage::AcmeMessage(_) => {
                format!("state.cluster.{}.acme", self.cluster.subject_name())
            }
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
