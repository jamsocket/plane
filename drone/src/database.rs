//! Interface to the shared sqlite database.
//!
//! All interaction between the proxy and the agent happen
//! asynchronously by updating the state of the sqlite
//! database.
//!
//! Queries are type-checked by the Rust compiler using sqlx,
//! based on type information stored in `sqlx-data.json`. If
//! you change a query in this file, you will likely need to
//! run `generate-sqlx-data.mjs` to get Rust to accept it.
use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use dis_spawner::{
    messages::agent::{BackendState, SpawnRequest},
    types::BackendId,
};
use sqlx::{Result, SqlitePool};

#[allow(unused)]
#[derive(Clone, Debug)]
pub struct DroneDatabase {
    pool: SqlitePool,
}

pub struct Backend {
    pub backend_id: BackendId,
    pub state: BackendState,
    pub spec: SpawnRequest,
}

#[allow(unused)]
impl DroneDatabase {
    pub fn new(pool: SqlitePool) -> DroneDatabase {
        DroneDatabase { pool }
    }

    pub async fn insert_backend(&self, spec: &SpawnRequest) -> Result<()> {
        let backend_id = spec.backend_id.id().to_string();
        let spec =
            serde_json::to_string(&spec).expect("SpawnRequest serialization should never fail.");

        sqlx::query!(
            r"
            insert into backend
            (name, spec, state)
            values
            (?, ?, 'Loading')
            ",
            backend_id,
            spec,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_backends(&self) -> anyhow::Result<Vec<Backend>> {
        sqlx::query!(
            r"
            select name, spec, state
            from backend
            "
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|d| {
            Ok(Backend {
                backend_id: BackendId::new(d.name.clone()),
                spec: serde_json::from_str(&d.spec)?,
                state: BackendState::from_str(&d.state)?,
            })
        })
        .collect()
    }

    pub async fn update_backend_state(
        &self,
        backend: &BackendId,
        state: BackendState,
    ) -> Result<()> {
        let state = state.to_string();

        sqlx::query!(
            r"
            update backend
            set state = ?
            ",
            state
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get the downstream source to direct a request on an incoming subdomain to.
    pub async fn get_proxy_route(&self, subdomain: &str) -> Result<Option<String>> {
        Ok(sqlx::query!(
            r"
            select address
            from route
            where subdomain = ?
            ",
            subdomain
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|d| d.address))
    }

    pub async fn insert_proxy_route(
        &self,
        backend: &BackendId,
        subdomain: &str,
        address: &str,
    ) -> Result<()> {
        let backend_id = backend.id().to_string();
        sqlx::query!(
            r"
            insert into route
            (backend, subdomain, address, last_active)
            values
            (?, ?, ?, unixepoch())
            ",
            backend_id,
            subdomain,
            address
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn reset_last_active_times(&self, subdomains: &[String]) -> Result<()> {
        for subdomain in subdomains {
            sqlx::query!(
                r"
                update route
                set last_active = unixepoch()
                where subdomain = ?
                ",
                subdomain
            )
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    pub async fn get_backend_last_active(&self, backend: &BackendId) -> Result<DateTime<Utc>> {
        let backend_id = backend.id();

        let time = sqlx::query!(
            r#"
            select last_active
            from route
            where backend = ?
            "#,
            backend_id
        )
        .fetch_one(&self.pool)
        .await?
        .last_active;

        Ok(Utc.timestamp(time, 0))
    }
}
