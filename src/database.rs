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

use sqlx::{SqlitePool, Result};

#[allow(unused)]
#[derive(Clone)]
pub struct DroneDatabase {
    pool: SqlitePool,
}

#[allow(unused)]
impl DroneDatabase {
    pub fn new(pool: SqlitePool) -> DroneDatabase {
        DroneDatabase {
            pool
        }
    }

    /// Get the downstream source to direct a request on an incoming subdomain to.
    pub async fn get_proxy_route(&self, subdomain: &str) -> Result<Option<String>> {
        Ok(sqlx::query!(r"
            select dest_address
            from route
            where subdomain = ?
        ", subdomain).fetch_optional(&self.pool).await?.map(|d| d.dest_address))
    }
}