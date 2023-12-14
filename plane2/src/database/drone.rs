use crate::{
    heartbeat_consts::UNHEALTHY_SECONDS,
    types::{ClusterId, NodeId},
};
use sqlx::{postgres::types::PgInterval, query, PgPool};
use std::time::Duration;

pub struct DroneDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> DroneDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn register_drone(&self, id: NodeId) -> sqlx::Result<()> {
        query!(
            r#"
            insert into drone (id, draining)
            values ($1, false)
            on conflict do nothing
            "#,
            id.as_i32(),
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

    /// TODO: simple algorithm until we collect more metrics.
    pub async fn pick_drone_for_spawn(&self, cluster: &ClusterId) -> sqlx::Result<Option<NodeId>> {
        let result = query!(
            r#"
            select
                drone.id
            from node
            left join drone
            on node.id = drone.id
            where
                last_status = 'Available'
                and controller is not null
                and cluster = $1
                and now() - last_heartbeat < $2
                and draining = false
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

        Ok(result.map(|r| NodeId::from(r.id)))
    }
}
