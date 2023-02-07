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
use chrono::{DateTime, TimeZone, Utc};
use plane_core::{
    messages::agent::{BackendState, SpawnRequest},
    types::BackendId,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{migrate, Result, SqlitePool};
use std::{path::Path, str::FromStr};

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

pub struct ProxyRoute {
    pub address: String,
    pub bearer_token: Option<String>,
}

#[allow(unused)]
impl DroneDatabase {
    pub async fn new(db_path: &Path) -> Result<DroneDatabase> {
        let co = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new().connect_with(co).await?;
        migrate!("./migrations").run(&pool).await?;

        let connection = DroneDatabase { pool };
        Ok(connection)
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

    pub async fn running_backends(&self) -> anyhow::Result<i32> {
        let result = sqlx::query!(
            r"
            select count(1) as c from backend
            where state in ('Loading', 'Starting', 'Ready')
            "
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.c)
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
        let backend_id = backend.id().to_string();

        sqlx::query!(
            r"
            update backend
            set state = ?
            where name = ?
            ",
            state,
            backend_id,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get the downstream source to direct a request on an incoming subdomain to.
    pub async fn get_proxy_route(&self, subdomain: &str) -> Result<Option<ProxyRoute>> {
        sqlx::query_as!(
            ProxyRoute,
            r"
            select address, bearer_token
            from route
            left join backend
            on route.backend = backend.name
            where subdomain = ?
            and state = 'Ready'
            ",
            subdomain
        )
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn insert_proxy_route(
        &self,
        backend: &BackendId,
        subdomain: &str,
        address: &str,
        bearer_token: Option<&str>,
    ) -> Result<()> {
        let backend_id = backend.id().to_string();
        sqlx::query!(
            r"
            insert or replace into route
            (backend, subdomain, address, last_active, bearer_token)
            values
            (?, ?, ?, unixepoch(), ?)
            ",
            backend_id,
            subdomain,
            address,
            bearer_token,
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

        Ok(Utc.timestamp_opt(time, 0).single().expect(
            "timestamp_opt should always return a LocalResult::Single for in-range inputs.",
        ))
    }
}
