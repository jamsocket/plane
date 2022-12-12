use self::backend_view::BackendView;
use crate::{
    messages::{agent::DroneState, state::StateUpdate},
    types::{BackendId, ClusterName, DroneId},
};
use std::{
    collections::{BTreeMap, HashMap},
    net::IpAddr,
    sync::{Arc, RwLock},
};
use time::OffsetDateTime;

pub mod backend_view;
pub mod replica;

#[derive(Default, Debug)]
pub struct DroneView {
    /// The most recent heartbeat we've received for this drone.
    pub last_heartbeat: Option<(DroneState, OffsetDateTime)>,
    pub ip: Option<IpAddr>,
    pub version: Option<String>,

    /// A map of backends.
    pub backends: HashMap<BackendId, Arc<RwLock<BackendView>>>,
}

impl DroneView {
    pub fn state(&self) -> Option<DroneState> {
        let (last_drone_state, _timestamp) = self.last_heartbeat?;
        Some(last_drone_state)
    }

    pub fn num_backends(&self) -> usize {
        // Count the number of backends that are not in a terminal state.
        // TODO: This is O(n) in the number of backends. We could make it O(1) by
        // keeping a separate counter.
        self.backends
            .iter()
            .filter(|(_, backend)| {
                backend
                    .read()
                    .unwrap()
                    .state()
                    .map(|d| !d.terminal())
                    .unwrap_or_default()
            })
            .count()
    }
}

#[derive(Default)]
pub struct ClusterView {
    /// Maps active backends to the drone they run on.
    pub routes: BTreeMap<BackendId, DroneId>,

    /// Maps drones to their status.
    pub drones: BTreeMap<DroneId, Arc<RwLock<DroneView>>>,
}

impl ClusterView {
    fn update_state(&mut self, update: StateUpdate, timestamp: OffsetDateTime) {
        match update {
            StateUpdate::DroneStatus {
                drone,
                state,
                ip,
                drone_version,
                ..
            } => {
                let drone = self.drones.entry(drone).or_default();

                {
                    let mut drone = drone.write().unwrap();
                    drone.last_heartbeat = Some((state, timestamp));
                    drone.ip = Some(ip);
                    drone.version = Some(drone_version);
                }

                tracing::info!(?drone, "Drone status update");
            }
            StateUpdate::BackendStatus {
                drone,
                backend,
                state,
                ..
            } => {
                let mut drone_view = self
                    .drones
                    .entry(drone.clone())
                    .or_default()
                    .write()
                    .unwrap();
                if state.terminal() {
                    self.routes.remove(&backend);
                } else {
                    self.routes.insert(backend.clone(), drone);
                }

                drone_view
                    .backends
                    .entry(backend)
                    .or_default()
                    .write()
                    .unwrap()
                    .update_state(state, timestamp);
            }
        }
    }

    pub fn drone(&self, drone: &DroneId) -> Option<Arc<RwLock<DroneView>>> {
        self.drones.get(drone).cloned()
    }

    pub fn route(&self, backend: &BackendId) -> Option<IpAddr> {
        let drone_id = self.routes.get(backend)?;
        let drone = self.drone(drone_id)?;
        let drone = drone.read().unwrap();

        drone.ip
    }
}

#[derive(Default)]
pub struct SystemView {
    pub clusters: BTreeMap<ClusterName, ClusterView>,
}

impl SystemView {
    pub fn update_state(&mut self, update: StateUpdate, timestamp: OffsetDateTime) {
        let cluster_name = update.cluster();
        let cluster = self.clusters.entry(cluster_name.clone()).or_default();
        cluster.update_state(update, timestamp);
    }

    pub fn cluster(&self, cluster: &ClusterName) -> Option<&ClusterView> {
        self.clusters.get(cluster)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::messages::{agent::BackendState, PLANE_VERSION};
    use std::net::Ipv4Addr;

    #[test]
    fn test_drone_status() {
        let mut system = SystemView::default();
        let cluster = ClusterName::new("plane.test");
        let drone = DroneId::new_random();

        // Drone view is created when status is first seen.
        system.update_state(
            StateUpdate::DroneStatus {
                cluster: cluster.clone(),
                drone: drone.clone(),
                state: DroneState::Starting,
                ip: IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
                drone_version: PLANE_VERSION.into(),
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        assert_eq!(
            DroneState::Starting,
            system
                .cluster(&cluster)
                .expect("Expected to find cluster.")
                .drone(&drone)
                .expect("Expected to find drone.")
                .read()
                .unwrap()
                .state()
                .expect("Expected state.")
        );

        // Drone view is updated when the drone is stopped.
        system.update_state(
            StateUpdate::DroneStatus {
                cluster: cluster.clone(),
                drone: drone.clone(),
                state: DroneState::Stopped,
                ip: IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
                drone_version: PLANE_VERSION.into(),
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        assert_eq!(
            DroneState::Stopped,
            system
                .cluster(&cluster)
                .expect("Expected to find cluster.")
                .drone(&drone)
                .expect("Expected to find drone.")
                .read()
                .unwrap()
                .state()
                .expect("Expected state.")
        );
    }

    #[test]
    fn test_route() {
        let mut system = SystemView::default();
        let cluster = ClusterName::new("plane.test");
        let drone = DroneId::new_random();
        let backend = BackendId::new_random();
        let ip = IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12));

        // Drone view is created when status is first seen.
        system.update_state(
            StateUpdate::BackendStatus {
                cluster: cluster.clone(),
                drone: drone.clone(),
                backend: backend.clone(),
                state: BackendState::Ready,
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        // Route will not work yet, because we have not yet seen
        // a drone status message from this drone.
        assert!(system
            .cluster(&cluster)
            .expect("Expected cluster")
            .route(&backend)
            .is_none());

        system.update_state(
            StateUpdate::DroneStatus {
                cluster: cluster.clone(),
                drone,
                state: DroneState::Ready,
                ip,
                drone_version: PLANE_VERSION.into(),
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        // Now we have seen a drone status message, so the route
        // works.
        assert_eq!(
            ip,
            system
                .cluster(&cluster)
                .expect("Expected cluster")
                .route(&backend)
                .expect("Expected route")
        );
    }

    #[test]
    fn test_num_backends() {
        let mut system = SystemView::default();
        let cluster = ClusterName::new("plane.test");
        let drone = DroneId::new_random();
        let backend = BackendId::new_random();
        let ip = IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12));

        system.update_state(
            StateUpdate::DroneStatus {
                cluster: cluster.clone(),
                drone: drone.clone(),
                state: DroneState::Ready,
                ip,
                drone_version: PLANE_VERSION.into(),
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        assert_eq!(
            0,
            system
                .cluster(&cluster)
                .expect("Expected cluster.")
                .drone(&drone)
                .expect("Expected drone")
                .read()
                .unwrap()
                .num_backends()
        );

        system.update_state(
            StateUpdate::BackendStatus {
                cluster: cluster.clone(),
                drone: drone.clone(),
                backend: backend.clone(),
                state: BackendState::Starting,
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        assert_eq!(
            1,
            system
                .cluster(&cluster)
                .expect("Expected cluster.")
                .drone(&drone)
                .expect("Expected drone")
                .read()
                .unwrap()
                .num_backends()
        );

        system.update_state(
            StateUpdate::BackendStatus {
                cluster: cluster.clone(),
                drone: drone.clone(),
                backend,
                state: BackendState::Swept,
            },
            OffsetDateTime::UNIX_EPOCH,
        );

        assert_eq!(
            0,
            system
                .cluster(&cluster)
                .expect("Expected cluster.")
                .drone(&drone)
                .expect("Expected drone")
                .read()
                .unwrap()
                .num_backends()
        );
    }
}
