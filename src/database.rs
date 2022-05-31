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

use sqlx::{Result, SqlitePool};

#[allow(unused)]
#[derive(Clone)]
pub struct DroneDatabase {
    pool: SqlitePool,
}

#[allow(unused)]
impl DroneDatabase {
    pub fn new(pool: SqlitePool) -> DroneDatabase {
        DroneDatabase { pool }
    }

    /// Get the downstream source to direct a request on an incoming subdomain to.
    pub async fn get_proxy_route(&self, backend: &str) -> Result<Option<String>> {
        Ok(sqlx::query!(
            r"
            select address
            from route
            where backend = ?
            ",
            backend
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|d| d.address))
    }

    pub async fn reset_last_active_times(&self, backends: &[String]) -> Result<()> {
        for backend in backends {
            sqlx::query!(
                r"
            update route
            set last_active = unixepoch()
            where backend = ?
            ",
                backend
            )
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }
}
