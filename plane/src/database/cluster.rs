use crate::{
    names::{AnyNodeName, ControllerName},
    types::{ClusterName, ClusterState, DroneState, NodeState},
};
use anyhow::Context;
use sqlx::PgPool;

pub struct ClusterDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> ClusterDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn cluster_state(&self, cluster: &ClusterName) -> anyhow::Result<ClusterState> {
        // TODO: store created timestamp of nodes and use that for order.
        let result = sqlx::query!(
            r#"
            select
                node.name as "name!",
                node.kind as "node_kind!",
                node.plane_version as "plane_version!",
                node.plane_hash as "plane_hash!",
                node.controller as "controller!",
                drone.ready as "ready?",
                drone.draining as "draining?",
                drone.last_heartbeat as "last_drone_heartbeat",
                controller.last_heartbeat as "last_controller_heartbeat!",
                now() as "as_of!",
                (
                    select count(1)
                    from backend
                    where backend.drone_id = drone.id
                    and backend.last_status != 'terminated'
                ) as "backend_count"
            from node
            left join drone on node.id = drone.id
            left join controller on node.controller = controller.id
            where node.cluster = $1
            and node.controller is not null
            order by node.id asc
            "#,
            cluster.to_string()
        )
        .fetch_all(self.pool)
        .await?;

        let mut drones: Vec<DroneState> = Vec::new();
        let mut proxies: Vec<NodeState> = Vec::new();

        for node in result {
            let controller_heartbeat_age = node.as_of - node.last_controller_heartbeat;

            let node_state = NodeState {
                name: AnyNodeName::try_from(node.name)?,
                plane_version: node.plane_version,
                plane_hash: node.plane_hash,
                controller: ControllerName::try_from(node.controller)?,
                controller_heartbeat_age,
            };

            match node.node_kind.as_str() {
                "Drone" => {
                    let drone_state = DroneState {
                        ready: node.ready.context("Drone should have ready")?,
                        draining: node.draining.context("Drone should have draining")?,
                        backend_count: node
                            .backend_count
                            .context("Drone should have backend_count")?
                            as u32,
                        last_heartbeat_age: node.as_of
                            - node
                                .last_drone_heartbeat
                                .context("Drone should have last_heartbeat")?,
                        node: node_state,
                    };
                    drones.push(drone_state);
                }
                "Proxy" => {
                    proxies.push(node_state);
                }
                _ => {
                    // DNS servers are nodes, but they won't appear here because they can't (currently)
                    // be associated with a cluster.
                    tracing::warn!("Unknown node kind: {}", node.node_kind);
                }
            }
        }

        Ok(ClusterState { proxies, drones })
    }
}
