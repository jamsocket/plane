use super::{
    async_drop::AsyncDrop,
    resources::{database::DevDatabase, pebble::Pebble},
};
use bollard::Docker;
use plane2::{
    controller::ControllerServer,
    database::PlaneDatabase,
    dns::run_dns_with_listener,
    drone::Drone,
    names::{AcmeDnsServerName, ControllerName, DroneName, Name},
    types::ClusterName,
    util::random_string,
};
use std::{
    net::Ipv4Addr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tracing::subscriber::DefaultGuard;
use tracing_appender::non_blocking::WorkerGuard;

const TEST_CLUSTER: &str = "plane.test";

#[derive(Clone)]
pub struct TestEnvironment {
    pub scratch_dir: PathBuf,
    db: Arc<Mutex<Option<DevDatabase>>>,
    drop_futures: Arc<Mutex<Vec<Arc<dyn AsyncDrop>>>>,
    log_subscription: Arc<Mutex<Option<LogSubscription>>>,
    pub run_name: String,
    pub cluster: ClusterName,
}

#[allow(dead_code)]
impl TestEnvironment {
    pub fn new(name: &str) -> Self {
        let scratch_dir = create_scratch_dir(name);
        let run_name = random_string();

        let log_subscription = LogSubscription::subscribe(scratch_dir.clone());

        Self {
            db: Arc::default(),
            drop_futures: Arc::default(),
            scratch_dir,
            log_subscription: Arc::new(Mutex::new(Some(log_subscription))),
            cluster: ClusterName::from(TEST_CLUSTER.to_string()),
            run_name,
        }
    }

    pub async fn cleanup(&self) {
        // Stop the log subscription before dumping the database
        // (otherwise, it logs a bunch of things that are not related to the test.)
        self.log_subscription.lock().unwrap().take();

        // Dump the database.
        if let Some(db) = self.db.lock().unwrap().take() {
            db.drop_future().await.unwrap();
        }

        // Drop anything that registered a future.
        let new_drop_futures = {
            let mut drop_futures_lock = self.drop_futures.lock().unwrap();
            std::mem::take(&mut *drop_futures_lock)
        };

        for drop_future in new_drop_futures {
            drop_future.drop_future().await.unwrap();
        }
    }

    pub async fn db(&mut self) -> PlaneDatabase {
        let mut db_lock = self.db.lock().unwrap();
        if db_lock.is_none() {
            let db = DevDatabase::start(&self).await.unwrap();
            *db_lock = Some(db);
        }

        let dev_db = db_lock.as_ref().unwrap();
        dev_db.db.clone()
    }

    pub async fn controller(&mut self) -> ControllerServer {
        let db = self.db().await;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let controller =
            ControllerServer::run_with_listener(db.clone(), listener, ControllerName::new_random())
                .await
                .expect("Unable to construct controller.");
        controller
    }

    pub async fn drone(&mut self, controller: &ControllerServer) -> Drone {
        let client = controller.client();
        let connector = client.drone_connection(&ClusterName::from(TEST_CLUSTER.to_string()));
        let docker = Docker::connect_with_local_defaults().unwrap();
        let db_path = self.scratch_dir.join("drone.db");
        Drone::run(
            &DroneName::new_random(),
            connector,
            docker,
            Ipv4Addr::LOCALHOST.into(),
            Some(&db_path),
            None,
        )
        .await
        .unwrap()
    }

    pub async fn dns(&mut self, controller: &ControllerServer) -> DnsServer {
        let client = controller.client();
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let name = AcmeDnsServerName::new_random();
        let handle = tokio::spawn(async move {
            run_dns_with_listener(name, client, listener).await.unwrap();
        });

        DnsServer {
            port,
            _handle: Some(handle),
        }
    }

    pub async fn pebble(&mut self, dns_port: u16) -> Arc<Pebble> {
        let pebble = Arc::new(Pebble::new(&self, dns_port).await.unwrap());
        self.drop_futures.lock().unwrap().push(pebble.clone());
        pebble
    }
}

pub struct DnsServer {
    pub port: u16,
    _handle: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for DnsServer {
    fn drop(&mut self) {
        self._handle.take().unwrap().abort();
    }
}

fn create_scratch_dir(name: &str) -> PathBuf {
    let scratch_dir = PathBuf::from(format!("../test-scratch/{}", name));
    std::fs::remove_dir_all(&scratch_dir).unwrap_or(());
    std::fs::create_dir_all(&scratch_dir).unwrap();
    scratch_dir
}

struct LogSubscription {
    _default_guard: DefaultGuard,
    _worker_guard: WorkerGuard,
}

impl LogSubscription {
    pub fn subscribe(path: PathBuf) -> Self {
        let file_appender = tracing_appender::rolling::RollingFileAppender::new(
            tracing_appender::rolling::Rotation::NEVER,
            path,
            "test-log.txt",
        );

        let (non_blocking, _worker_guard) = tracing_appender::non_blocking(file_appender);

        let subscriber = tracing_subscriber::fmt()
            .compact()
            .with_ansi(false)
            .with_writer(non_blocking)
            .finish();

        let dispatcher = tracing::dispatcher::Dispatch::new(subscriber);
        let _default_guard = tracing::dispatcher::set_default(&dispatcher);

        Self {
            _default_guard,
            _worker_guard,
        }
    }
}
