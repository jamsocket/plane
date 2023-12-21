use crate::{
    heartbeat_consts::UNHEALTHY_SECONDS,
    types::{ClusterName, NodeId},
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::types::PgInterval, query, query_as, PgPool};
use std::time::{Duration, SystemTime};

pub struct DroneDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> DroneDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn register_drone(&self, id: NodeId, ready: bool) -> sqlx::Result<()> {
        query!(
            r#"
            insert into drone (id, draining, ready)
            values ($1, false, $2)
            on conflict (id) do update set
                ready = $2
            "#,
            id.as_i32(),
            ready,
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn drain(&self, id: NodeId) -> sqlx::Result<()> {
        let result = query!(
            r#"
            update drone
            set draining = true
            where id = $1
            "#,
            id.as_i32(),
        )
        .execute(self.pool)
        .await?;

        if result.rows_affected() == 0 {
            Err(sqlx::Error::RowNotFound)
        } else {
            Ok(())
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

    /// TODO: simple algorithm until we collect more metrics.
    pub async fn pick_drone_for_spawn(
        &self,
        cluster: &ClusterName,
    ) -> sqlx::Result<Option<DroneForSpawn>> {
        let result = query_as!(
            DroneForSpawn,
            r#"
            select
                drone.id,
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
            order by (
                select
                    count(*)
                from backend
                where drone_id = node.id
                and last_status != 'Terminated'
            ) asc, random()
            limit 1
            "#,
            cluster.to_string(),
            PgInterval::try_from(Duration::from_secs(UNHEALTHY_SECONDS as _))
                .expect("valid interval"),
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(result)
    }
}

pub struct DroneForSpawn {
    pub id: NodeId,
    pub last_local_time: SystemTime,
}
