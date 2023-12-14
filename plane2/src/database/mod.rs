use self::{
    acme::AcmeDatabase,
    backend::BackendDatabase,
    backend_actions::BackendActionDatabase,
    backend_key::KeysDatabase,
    connect::ConnectError,
    controller::ControllerDatabase,
    drone::DroneDatabase,
    node::NodeDatabase,
    subscribe::{EventSubscriptionManager, Notification, NotificationPayload, Subscription},
};
use crate::types::{ClusterId, ConnectRequest, ConnectResponse};
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast::Receiver;

pub mod acme;
pub mod backend;
pub mod backend_actions;
pub mod backend_key;
pub mod connect;
pub mod controller;
pub mod drone;
pub mod node;
pub mod subscribe;
pub mod util;

pub async fn connect_and_migrate(db: &str) -> sqlx::Result<PlaneDatabase> {
    let db_pool = PgPoolOptions::new().max_connections(5).connect(db).await?;
    sqlx::migrate!("schema/migrations").run(&db_pool).await?;
    Ok(PlaneDatabase::new(db_pool))
}

pub async fn connect(db: &str) -> sqlx::Result<PlaneDatabase> {
    let db_pool = PgPoolOptions::new().max_connections(1).connect(db).await?;
    Ok(PlaneDatabase::new(db_pool))
}

#[derive(Clone)]
pub struct PlaneDatabase {
    pool: PgPool,
    subscription_manager: Arc<OnceLock<EventSubscriptionManager>>,
}

impl PlaneDatabase {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            subscription_manager: Arc::default(),
        }
    }

    pub fn acme(&self) -> acme::AcmeDatabase {
        AcmeDatabase::new(&self.pool)
    }

    pub fn drone(&self) -> DroneDatabase {
        DroneDatabase::new(&self.pool)
    }

    pub fn node(&self) -> NodeDatabase {
        NodeDatabase::new(&self.pool)
    }

    pub fn backend(&self) -> BackendDatabase {
        BackendDatabase::new(self)
    }

    pub fn backend_actions(&self) -> BackendActionDatabase {
        BackendActionDatabase::new(&self.pool)
    }

    pub fn locks(&self) -> backend_key::KeysDatabase {
        KeysDatabase::new(&self.pool)
    }

    pub fn controller(&self) -> controller::ControllerDatabase {
        ControllerDatabase::new(&self.pool)
    }

    pub async fn connect(
        &self,
        cluster: &ClusterId,
        request: &ConnectRequest,
    ) -> Result<ConnectResponse, ConnectError> {
        connect::connect(&self.pool, cluster, request).await
    }

    fn subscription_manager(&self) -> &EventSubscriptionManager {
        self.subscription_manager
            .get_or_init(|| EventSubscriptionManager::new(&self.pool))
    }

    pub fn subscribe<T: NotificationPayload>(&self) -> Subscription<T> {
        self.subscription_manager().subscribe(None)
    }

    pub fn subscribe_with_key<T: NotificationPayload>(&self, key: &str) -> Subscription<T> {
        self.subscription_manager().subscribe(Some(key))
    }

    pub fn subscribe_all_events(&self) -> Receiver<Notification<Value>> {
        self.subscription_manager().subscribe_all_events()
    }
}
