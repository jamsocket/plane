use crate::PLANE_GIT_HASH;
use crate::PLANE_VERSION;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use plane_client::names::ControllerName;
use sqlx::PgPool;

pub struct ControllerDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> ControllerDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn heartbeat(&self, name: &ControllerName, is_online: bool) -> Result<()> {
        // Putting the insert and notification in the same transaction ensures that both
        // are given the exact same timestamp.
        let mut transaction = self.pool.begin().await?;

        sqlx::query!(
            r#"
            insert into controller (id, is_online, plane_version, plane_hash, last_heartbeat, ip)
            values ($1, $2, $3, $4, now(), inet_client_addr())
            on conflict (id) do update
            set
                is_online = $2,
                plane_version = $3,
                plane_hash = $4,
                last_heartbeat = now(),
                ip = inet_client_addr()
            "#,
            name.to_string(),
            is_online,
            PLANE_VERSION,
            PLANE_GIT_HASH,
        )
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(())
    }

    pub async fn online_controllers(&self) -> Result<Vec<Controller>, sqlx::Error> {
        let mut controllers = Vec::new();

        let result = sqlx::query!(
            r#"
            select
                id,
                is_online,
                plane_version,
                plane_hash,
                first_seen,
                last_heartbeat,
                now() as "as_of!"
            from controller
            where is_online = true
            "#,
        )
        .fetch_all(self.pool)
        .await?;

        for row in result {
            let controller = Controller {
                id: ControllerName::try_from(row.id)
                    .map_err(|_| sqlx::Error::Decode("Failed to decode controller name.".into()))?,
                is_online: row.is_online,
                plane_version: row.plane_version,
                plane_hash: row.plane_hash,
                first_seen: row.first_seen,
                last_heartbeat: row.last_heartbeat,
                as_of: row.as_of,
            };

            controllers.push(controller);
        }

        Ok(controllers)
    }
}

pub struct Controller {
    pub id: ControllerName,
    pub is_online: bool,
    pub plane_version: String,
    pub plane_hash: String,
    pub first_seen: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    pub as_of: DateTime<Utc>,
}
