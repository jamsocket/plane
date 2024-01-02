//! ```
//! use std::time::Duration;
//! use plane::database::backend_key::*;
//! assert!(KEY_LEASE_RENEW_AFTER > Duration::from_secs(0));
//! assert!(KEY_LEASE_SOFT_TERMINATE_AFTER > KEY_LEASE_RENEW_AFTER);
//! assert!(KEY_LEASE_HARD_TERMINATE_AFTER > KEY_LEASE_SOFT_TERMINATE_AFTER);
//! assert!(KEY_LEASE_EXPIRATION > KEY_LEASE_HARD_TERMINATE_AFTER);
//! ```

use crate::{
    names::BackendName,
    types::{ClusterName, KeyConfig},
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

    pub async fn prevent_renew(&self, backend: &BackendName) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            update backend_key
            set allow_renew = false
            where
                id = $1 and
                allow_renew = true
            "#,
            backend.to_string(),
        )
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Remove a key, ensuring that it is still expired.
    /// Returns Ok(true) if the key was successfully removed.
    pub async fn remove_key(&self, backend: BackendName) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            delete from backend_key
            where id = $1
            and expires_at < now()
            "#,
            backend.to_string(),
        )
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn renew_key(&self, id: &BackendName) -> Result<(), sqlx::Error> {
        let result = sqlx::query!(
            r#"
            update backend_key
            set expires_at = now() + $2
            where
                id = $1 and
                allow_renew = true
            "#,
            id.to_string(),
            PgInterval::try_from(KEY_LEASE_EXPIRATION).expect("valid constant interval"),
        )
        .execute(self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }

        Ok(())
    }

    /// Checks if the key is held.
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
                backend_key.expires_at as expires_at,
                backend_key.fencing_token as token,
                backend_key.key_name as name,
                now() as "as_of!"
            from backend_key
            left join backend on backend_key.id = backend.id
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
                id: BackendName::try_from(lock_result.id)
                    .map_err(|_| sqlx::Error::Decode("Invalid backend name.".into()))?,
                token: lock_result.token,
                key: lock_result.name,
                tag: lock_result.tag,
                expires_at: lock_result.expires_at,
                as_of: lock_result.as_of,
            }))
        } else {
            Ok(None)
        }
    }
}

pub struct BackendKeyResult {
    pub id: BackendName,
    pub tag: String,
    pub token: i64,
    pub key: String,
    expires_at: DateTime<Utc>,
    as_of: DateTime<Utc>,
}

impl BackendKeyResult {
    pub fn is_live(&self) -> bool {
        self.as_of > self.expires_at
    }
}
