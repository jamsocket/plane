use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use plane_core::{
    messages::{
        agent,
        state::{BackendMessageType, ClusterStateMessage, DroneMessageType, WorldStateMessage},
    },
    types::{BackendId, ClusterName, DroneId},
};
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

#[derive(Default, Debug, Clone)]
pub struct StateHandle(Arc<RwLock<WorldState>>);

impl StateHandle {
    pub fn new(world_state: WorldState) -> Self {
        Self(Arc::new(RwLock::new(world_state)))
    }

    pub fn get_ready_drones(
        &self,
        cluster: &ClusterName,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<DroneId>> {
        let world_state = self.0.write().expect("Could not acquire world_state lock.");

        let min_keepalive = timestamp - chrono::Duration::seconds(30);

        tracing::info!(?world_state, "World state");

        Ok(world_state
            .cluster(cluster)
            .ok_or_else(|| anyhow!("Cluster not found."))?
            .drones
            .iter()
            .filter(|(_, drone)| {
                drone.state == Some(agent::DroneState::Ready)
                    && drone.ip.is_some()
                    && drone.last_seen > Some(min_keepalive)
            })
            .map(|(drone_id, _)| drone_id.clone())
            .collect())
    }

    pub fn state(&self) -> RwLockReadGuard<WorldState> {
        self.0.read().expect("Could not acquire world_state lock.")
    }

    pub fn write_state(&self) -> RwLockWriteGuard<WorldState> {
        self.0.write().expect("Could not acquire world_state lock.")
    }
}

#[derive(Default, Debug)]
pub struct WorldState {
    clusters: HashMap<ClusterName, ClusterState>,
}

impl WorldState {
    pub fn apply(&mut self, message: WorldStateMessage) {
        let cluster = self.clusters.entry(message.cluster.clone()).or_default();
        cluster.apply(message.message);
    }

    pub fn cluster(&self, cluster: &ClusterName) -> Option<&ClusterState> {
        self.clusters.get(cluster)
    }
}

#[derive(Default, Debug)]
pub struct ClusterState {
    pub drones: HashMap<DroneId, DroneState>,
    pub backends: HashMap<BackendId, BackendState>,
    pub txt_records: VecDeque<String>,
}

impl ClusterState {
    fn apply(&mut self, message: ClusterStateMessage) {
        match message {
            ClusterStateMessage::DroneMessage(message) => {
                let drone = self.drones.entry(message.drone).or_default();
                drone.apply(message.message);
            }
            ClusterStateMessage::BackendMessage(message) => {
                let backend = self.backends.entry(message.backend).or_default();
                backend.apply(message.message);
            }
            ClusterStateMessage::AcmeMessage(message) => {
                if self.txt_records.len() > 10 {
                    self.txt_records.pop_front();
                }

                self.txt_records.push_back(message.value);
            }
        }
    }

    pub fn a_record_lookup(&self, backend: &BackendId) -> Option<IpAddr> {
        let backend = self.backends.get(backend)?;

        let drone = backend.drone.as_ref()?;
        let drone = self.drones.get(&drone)?;

        drone.ip
    }
}

#[derive(Default, Debug)]
pub struct DroneState {
    pub ip: Option<IpAddr>,
    pub state: Option<agent::DroneState>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
}

impl DroneState {
    fn apply(&mut self, message: DroneMessageType) {
        match message {
            DroneMessageType::Metadata { ip } => self.ip = Some(ip),
            DroneMessageType::State { state } => self.state = Some(state),
            DroneMessageType::KeepAlive { timestamp } => self.last_seen = Some(timestamp),
        }
    }
}

#[derive(Default, Debug)]
pub struct BackendState {
    drone: Option<DroneId>,
    state: Option<agent::BackendState>,
}

impl BackendState {
    fn apply(&mut self, message: BackendMessageType) {
        match message {
            BackendMessageType::Assignment { drone } => self.drone = Some(drone),
            BackendMessageType::State { status } => self.state = Some(status),
        }
    }
}
