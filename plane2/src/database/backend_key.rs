use crate::{
    heartbeat_consts::{ASSUME_LOST_SECONDS, UNHEALTHY_SECONDS},
    names::BackendName,
    types::{BackendKeyId, ClusterName, KeyConfig},
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::types::PgInterval, PgPool};
use std::time::Duration;

pub const KEY_LEASE_RENEW_AFTER: Duration = Duration::from_secs(30);
pub const KEY_LEASE_SOFT_TERMINATE_AFTER: Duration = Duration::from_secs(40);
pub const KEY_LEASE_HARD_TERMINATE_AFTER: Duration = Duration::from_secs(50);
pub const KEY_LEASE_EXPIRATION: Duration = Duration::from_secs(60);

pub struct KeysDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> KeysDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Remove a key, ensuring that it is still expired.
    /// Returns Ok(true) if the key was successfully removed.
    pub async fn remove_key(&self, id: BackendKeyId) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            delete from backend_key
            where id = $1
            and last_renewed - now() > $2
            "#,
            id.as_i32(),
            PgInterval::try_from(Duration::from_secs(ASSUME_LOST_SECONDS as _))
                .expect("valid interval"),
        )
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn renew_key(&self, id: BackendKeyId) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            update backend_key
            set last_renewed = now()
            where id = $1
            "#,
            id.as_i32(),
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn check_key(
        &self,
        cluster: &ClusterName,
        key: &KeyConfig,
    ) -> Result<Option<BackendKeyResult>, sqlx::Error> {
        let lock_result = sqlx::query!(
            r#"
            select
                backend_key.id as id,
                backend_key.tag as tag,
                backend_key.last_renewed as last_renewed,
                backend.id as backend_id,
                now() as "as_of!"
            from backend_key
            left join backend on backend_key.backend_id = backend.id
            where backend_key.cluster = $1
            and backend_key.key_name = $2
            and backend_key.namespace = $3
            "#,
            cluster.to_string(),
            key.name,
            key.namespace,
        )
        .fetch_optional(self.pool)
        .await?;

        if let Some(lock_result) = lock_result {
            Ok(Some(BackendKeyResult {
                id: BackendKeyId::from(lock_result.id),
                tag: lock_result.tag,
                backend_id: BackendName::try_from(lock_result.backend_id)
                    .map_err(|_| sqlx::Error::Decode("Failed to decode backend name.".into()))?,
                last_renewed: lock_result.last_renewed,
                as_of: lock_result.as_of,
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug)]
pub enum BackendKeyHealth {
    /// The lock has been renewed within the normal interval.
    Active(BackendName),

    /// The lock has not been renewed within the normal interval (plus grace period),
    /// but it has not been inactive long enough to be removed.
    Unhealthy,

    /// The lock has expired.
    Expired,
}

pub struct BackendKeyResult {
    pub id: BackendKeyId,
    pub tag: String,
    backend_id: BackendName,
    last_renewed: DateTime<Utc>,
    as_of: DateTime<Utc>,
}

impl BackendKeyResult {
    pub fn key_health(&self) -> BackendKeyHealth {
        let key_age = (self.as_of - self.last_renewed).num_seconds();
        if key_age > ASSUME_LOST_SECONDS {
            return BackendKeyHealth::Expired;
        }
        if key_age > UNHEALTHY_SECONDS {
            return BackendKeyHealth::Unhealthy;
        }
        BackendKeyHealth::Active(self.backend_id.clone())
    }
}
