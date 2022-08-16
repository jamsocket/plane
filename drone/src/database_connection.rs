use crate::database::DroneDatabase;
use anyhow::Result;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct DatabaseConnection {
    db_path: String,
    connection: Arc<Mutex<Option<DroneDatabase>>>,
}

impl PartialEq for DatabaseConnection {
    fn eq(&self, other: &Self) -> bool {
        self.db_path == other.db_path
    }
}

impl DatabaseConnection {
    pub fn new(db_path: String) -> Self {
        DatabaseConnection {
            db_path,
            connection: Arc::default(),
        }
    }

    pub async fn connection(&self) -> Result<DroneDatabase> {
        let mut shared_connection = self.connection.lock().await;

        if let Some(connection) = shared_connection.as_ref() {
            Ok(connection.clone())
        } else {
            let co = SqliteConnectOptions::new()
                .filename(&self.db_path)
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new().connect_with(co).await?;
            migrate!("./migrations").run(&pool).await?;

            let connection = DroneDatabase::new(pool);
            shared_connection.replace(connection.clone());
            Ok(connection)
        }
    }
}
