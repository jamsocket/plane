use super::agent::{BackendState, DroneState};
use crate::{
    nats::{JetStreamable, NoReply, TypedMessage},
    types::{BackendId, ClusterName, DroneId},
};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use time::OffsetDateTime;

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
    /// A message sent by a consumer who thinks a drone may be dead. If the
    /// message comes back and the last timestamp of the drone in question
    /// is still the last heartbeat on record, the drone can be assumed dead.
    MaybeDead {
        cluster: ClusterName,
        drone: DroneId,
        last_timestamp: OffsetDateTime,
    },
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
            StateUpdate::MaybeDead { cluster, drone, .. } => format!(
                "cluster.{}.drone.{}.sm.dead",
                cluster.subject_name(),
                drone.id()
            ),
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
            subjects: vec!["cluster.*.drone.*.sm.>".into()],
            ..async_nats::jetstream::stream::Config::default()
        }
    }
}

impl StateUpdate {
    pub fn cluster(&self) -> &ClusterName {
        match self {
            StateUpdate::DroneStatus { cluster, .. } => cluster,
            StateUpdate::BackendStatus { cluster, .. } => cluster,
            StateUpdate::MaybeDead { cluster, .. } => cluster,
        }
    }
}
