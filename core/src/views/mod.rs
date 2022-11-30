use crate::{
    messages::{agent::{BackendState, DroneState}, state::StateUpdate},
    types::{BackendId, ClusterName, DroneId},
};
use std::{collections::HashMap, net::IpAddr};
use time::OffsetDateTime;

#[derive(Default)]
pub struct DroneView {
    /// The most recent heartbeat we've received for this drone.
    pub last_heartbeat: Option<(DroneState, OffsetDateTime)>,
    pub ip: Option<IpAddr>,
    pub version: Option<String>,

    /// A map of backends relevant to scheduling or routing.
    /// When backends terminate, they are removed from this map.
    pub backends: HashMap<BackendId, BackendState>,
}

impl DroneView {
    pub fn state(&self) -> Option<DroneState> {
        let (last_drone_state, _timestamp) = self.last_heartbeat?;
        Some(last_drone_state)
    }

    pub fn num_backends(&self) -> usize {
        self.backends.len()
    }
}

#[derive(Default)]
pub struct ClusterView {
    /// Maps active backends to the drone they run on.
    backends: HashMap<BackendId, DroneId>,

    /// Maps drones to their status.
    drones: HashMap<DroneId, DroneView>,
}

impl ClusterView {
    pub fn update_state(&mut self, update: StateUpdate, timestamp: OffsetDateTime) {
        match update {
            StateUpdate::DroneStatus {
                drone,
                state,
                ip,
                drone_version,
                ..
            } => {
                if state == DroneState::Stopped {
                    self.drones.remove(&drone);
                    return;
                }
                let mut drone = self.drones.entry(drone.clone()).or_default();

                drone.last_heartbeat = Some((state, timestamp));
                drone.ip = Some(ip);
                drone.version = Some(drone_version);
            }
            StateUpdate::BackendStatus {
                drone,
                backend,
                state,
                ..
            } => {
                let drone_view = self.drones.entry(drone.clone()).or_default();
                if state.terminal() {
                    self.backends.remove(&backend);
                    drone_view.backends.remove(&backend);
                } else {
                    self.backends.insert(backend.clone(), drone);
                    drone_view.backends.insert(backend, state);
                }
            }
            StateUpdate::MaybeDead { .. } => todo!(),
        }
    }

    pub fn drone(&self, drone: &DroneId) -> Option<&DroneView> {
        self.drones.get(drone)
    }

    pub fn route(&self, backend: &BackendId) -> Option<IpAddr> {
        let drone_id = self.backends.get(backend)?;
        let drone = self.drone(drone_id)?;
        drone.ip
    }
}

#[derive(Default)]
pub struct SystemView {
    clusters: HashMap<ClusterName, ClusterView>,
}

impl SystemView {
    pub fn update_state(&mut self, update: StateUpdate, timestamp: OffsetDateTime) {
        let cluster = self.clusters.entry(update.cluster().clone()).or_default();
        cluster.update_state(update, timestamp);
    }

    pub fn cluster(&self, cluster: &ClusterName) -> Option<&ClusterView> {
        self.clusters.get(cluster)
    }
}

#[cfg(test)]
mod test {
    use std::net::Ipv4Addr;

    use super::*;
    const PLANE_VERSION: &str = env!("CARGO_PKG_VERSION");

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
                .state()
                .expect("Expected state.")
        );

        // Drone view is removed when the drone is stopped.
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

        assert!(system
            .cluster(&cluster)
            .expect("Expected to find cluster.")
            .drone(&drone)
            .is_none());
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
                .num_backends()
        );
    }
}
