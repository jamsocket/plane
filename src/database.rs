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

    pub async fn get_proxy_route(&self, subdomain: &str) -> Result<Option<String>> {
        Ok(sqlx::query!(r"
            select dest_address
            from route
            where subdomain = ?
        ", subdomain).fetch_optional(&self.pool).await?.map(|d| d.dest_address))
    }
}