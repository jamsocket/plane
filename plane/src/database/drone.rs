use crate::{
    heartbeat_consts::UNHEALTHY_SECONDS,
    names::{ControllerName, DroneName},
    types::{BackendStatus, ClusterName, DronePoolName, NodeId},
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::types::PgInterval, query, PgPool};
use std::str::FromStr;
use std::time::Duration;

pub struct DroneDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> DroneDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn register_drone(
        &self,
        id: NodeId,
        ready: bool,
        pool: DronePoolName,
    ) -> sqlx::Result<()> {
        query!(
            r#"
            insert into drone (id, draining, ready, pool)
            values ($1, false, $2, $3)
            on conflict (id) do update set
                ready = $2
            "#,
            id.as_i32(),
            ready,
            pool.to_string(),
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn drain(&self, id: NodeId) -> sqlx::Result<bool> {
        let result = query!(
            r#"
            update drone
            set draining = true
            where id = $1
            returning (
                select draining
                from drone
                where id = $1
            ) as "was_draining!"
            "#,
            id.as_i32(),
        )
        .fetch_optional(self.pool)
        .await?;

        if let Some(was_draining) = result {
            Ok(!was_draining.was_draining)
        } else {
            Err(sqlx::Error::RowNotFound)
        }
    }

    pub async fn heartbeat(&self, id: NodeId, local_time: DateTime<Utc>) -> sqlx::Result<()> {
        query!(
            r#"
            update drone
            set last_heartbeat = now(), last_local_time = $2
            where id = $1
            "#,
            id.as_i32(),
            local_time,
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_drone_pool(&self, id: NodeId) -> sqlx::Result<DronePoolName> {
        let result = query!(
            r#"
            select pool
            from drone
            where id = $1
            "#,
            id.as_i32(),
        )
        .fetch_one(self.pool)
        .await?;

        Ok(result.pool.into())
    }

    pub async fn get_active_drones_for_pool(
        &self,
        cluster: &ClusterName,
        pool: &DronePoolName,
    ) -> sqlx::Result<Vec<DroneWithMetadata>> {
        let result = query!(
            r#"
            select
                drone.id as id,
                drone.ready as ready,
                drone.draining as draining,
                drone.last_heartbeat as "last_heartbeat!",
                drone.last_local_time as "last_local_time!",
                drone.pool as pool,
                node.name as name,
                node.cluster as "cluster!",
                node.plane_version as plane_version,
                node.plane_hash as plane_hash,
                node.controller as "controller!",
                node.last_connection_start_time as "last_connection_start_time!"
            from node
            left join drone on node.id = drone.id
            where
                drone.ready = true
                and controller is not null
                and cluster = $1
                and now() - drone.last_heartbeat < $2
                and pool = $3
                and last_local_time is not null
                and last_connection_start_time is not null
            order by drone.id
            "#,
            cluster.to_string(),
            PgInterval::try_from(Duration::from_secs(UNHEALTHY_SECONDS as _))
                .expect("valid interval"),
            pool.to_string(),
        )
        .fetch_all(self.pool)
        .await?;

        let drones: Vec<DroneWithMetadata> = result
            .into_iter()
            .map(|r| DroneWithMetadata {
                id: NodeId::from(r.id),
                name: DroneName::try_from(r.name).expect("valid drone name"),
                ready: r.ready,
                draining: r.draining,
                last_heartbeat: r.last_heartbeat,
                last_local_time: r.last_local_time,
                pool: r.pool.into(),
                cluster: ClusterName::from_str(&r.cluster).expect("valid cluster name"),
                plane_version: r.plane_version,
                plane_hash: r.plane_hash,
                controller: ControllerName::try_from(r.controller).expect("valid controller name"),
                last_connection_start_time: r.last_connection_start_time,
            })
            .collect();

        Ok(drones)
    }

    /// TODO: simple algorithm until we collect more metrics.
    pub async fn pick_drone_for_spawn(
        &self,
        cluster: &ClusterName,
        pool: &DronePoolName,
    ) -> sqlx::Result<Option<DroneForSpawn>> {
        let result = query!(
            r#"
            select
                drone.id,
                node.name,
                drone.last_local_time as "last_local_time!"
            from node
            left join drone
                on node.id = drone.id
            left join controller
                on node.controller = controller.id
            where
                drone.ready = true
                and controller is not null
                and cluster = $1
                and now() - drone.last_heartbeat < $2
                and now() - controller.last_heartbeat < $2
                and controller.is_online = true
                and draining = false
                and last_local_time is not null
                and pool = $3
            order by (
                select
                    count(*)
                from backend
                where drone_id = node.id
                and last_status != $4
            ) asc, random()
            limit 1
            "#,
            cluster.to_string(),
            PgInterval::try_from(Duration::from_secs(UNHEALTHY_SECONDS as _))
                .expect("valid interval"),
            pool.to_string(),
            BackendStatus::Terminated.to_string(),
        )
        .fetch_optional(self.pool)
        .await?;

        let result = match result {
            Some(result) => {
                let id = NodeId::from(result.id);
                let drone = DroneName::try_from(result.name).expect("valid drone name");
                let last_local_time = result.last_local_time;

                Some(DroneForSpawn {
                    id,
                    drone,
                    last_local_time,
                })
            }
            None => return Ok(None),
        };

        Ok(result)
    }
}

pub struct DroneForSpawn {
    pub id: NodeId,
    pub drone: DroneName,
    pub last_local_time: DateTime<Utc>,
}

pub struct DroneWithMetadata {
    pub id: NodeId,
    pub name: DroneName,
    pub ready: bool,
    pub draining: bool,
    pub last_heartbeat: DateTime<Utc>,
    pub last_local_time: DateTime<Utc>,
    pub pool: DronePoolName,
    pub cluster: ClusterName,
    pub plane_version: String,
    pub plane_hash: String,
    pub controller: ControllerName,
    pub last_connection_start_time: DateTime<Utc>,
}
