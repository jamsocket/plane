use super::agent::{BackendState, DroneState};
use crate::{
    nats::{JetStreamable, NoReply, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::net::IpAddr;

#[derive(Debug, Serialize, Deserialize)]
pub enum StateUpdate {
    /// A drone heartbeat, sent periodically as long as the drone is alive.
    /// Also sent immediately when the state changes.
    DroneStatus {
        cluster: ClusterName,
        drone: DroneId,

        /// State for scheduling purposes.
        state: DroneState,

        /// Public IP of the drone.
        ip: IpAddr,
        drone_version: String,
    },
    /// A backend state change. Sent immediately when a backend state changes.
    BackendStatus {
        cluster: ClusterName,
        drone: DroneId,
        backend: BackendId,
        state: BackendState,
    },
    Landmark {
        uuid: uuid::Uuid,
    }
}

impl StateUpdate {
    pub fn landmark() -> StateUpdate {
        Self::Landmark { uuid: Uuid::new_v4() }
    }
}

impl TypedMessage for StateUpdate {
    type Response = NoReply;

    fn subject(&self) -> String {
        match self {
            StateUpdate::DroneStatus { cluster, drone, .. } => format!(
                "cluster.{}.drone.{}.sm.status",
                cluster.subject_name(),
                drone.id()
            ),
            StateUpdate::BackendStatus {
                cluster,
                drone,
                backend,
                ..
            } => format!(
                "cluster.{}.drone.{}.sm.backend.{}.status",
                cluster.subject_name(),
                drone.id(),
                backend.id()
            ),
            StateUpdate::Landmark { .. } => "state_update.landmark".into(),
        }
    }
}

impl JetStreamable for StateUpdate {
    fn stream_name() -> &'static str {
        "system_state"
    }

    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().into(),
            max_messages_per_subject: 1,
            subjects: vec!["cluster.*.drone.*.sm.>".into(), "state_update.landmark".into()],
            ..async_nats::jetstream::stream::Config::default()
        }
    }
}

impl StateUpdate {
    pub fn cluster(&self) -> Option<&ClusterName> {
        match self {
            StateUpdate::DroneStatus { cluster, .. } => Some(cluster),
            StateUpdate::BackendStatus { cluster, .. } => Some(cluster),
            StateUpdate::Landmark { .. } => None,
        }
    }
}
