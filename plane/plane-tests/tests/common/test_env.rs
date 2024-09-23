use super::{
    async_drop::AsyncDrop,
    resources::{database::DevDatabase, pebble::Pebble},
};
use chrono::Duration;
use plane::{
    controller::ControllerServer,
    database::PlaneDatabase,
    dns::run_dns_with_listener,
    drone::runtime::unix_socket::{MessageToClient, MessageToServer, UnixSocketRuntimeConfig},
    drone::{runtime::docker::DockerRuntimeConfig, Drone, DroneConfig, ExecutorConfig},
    names::{AcmeDnsServerName, ControllerName, DroneName, Name},
    proxy::AcmeEabConfiguration,
    typed_unix_socket::{server::TypedUnixSocketServer, WrappedMessage},
    types::{ClusterName, DronePoolName},
    util::random_string,
};
use std::{
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast::Receiver;
use tracing::subscriber::DefaultGuard;
use tracing_appender::non_blocking::WorkerGuard;
use url::Url;

const TEST_CLUSTER: &str = "plane.test";

#[derive(Clone)]
pub struct TestEnvironment {
    pub scratch_dir: PathBuf,
    db: Arc<tokio::sync::Mutex<Option<DevDatabase>>>, // held across await, see: https://tokio.rs/tokio/tutorial/shared-state#on-using-stdsyncmutex-and-tokiosyncmutex
    drop_futures: Arc<Mutex<Vec<Arc<dyn AsyncDrop>>>>,
    log_subscription: Arc<Mutex<Option<LogSubscription>>>,
    pub run_name: String,
    #[allow(dead_code)] // Used in tests.
    pub cluster: ClusterName,
    pub pool: DronePoolName,
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
            cluster: TEST_CLUSTER.parse().unwrap(),
            run_name,
            pool: DronePoolName::default(),
        }
    }

    pub async fn cleanup(&self) {
        // Stop the log subscription before dumping the database
        // (otherwise, it logs a bunch of things that are not related to the test.)
        self.log_subscription.lock().unwrap().take();

        // Dump the database.
        if let Some(db) = self.db.lock().await.take() {
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
        let mut db_lock = self.db.lock().await;
        if db_lock.is_none() {
            let db = DevDatabase::start(self)
                .await
                .expect("Error starting database.");
            *db_lock = Some(db);
        }

        let dev_db = db_lock.as_ref().unwrap();
        dev_db.db.clone()
    }

    pub async fn controller(&mut self) -> ControllerServer {
        let db = self.db().await;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url: Url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();

        ControllerServer::run_with_listener(
            db.clone(),
            listener,
            ControllerName::new_random(),
            url,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Unable to construct controller.")
    }

    pub async fn controller_with_forward_auth(&mut self, forward_auth: &Url) -> ControllerServer {
        let db = self.db().await;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url: Url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();

        ControllerServer::run_with_listener(
            db.clone(),
            listener,
            ControllerName::new_random(),
            url,
            None,
            None,
            None,
            Some(forward_auth.clone()),
        )
        .await
        .expect("Unable to construct controller.")
    }

    pub async fn drone_internal(
        &mut self,
        controller: &ControllerServer,
        pool: &DronePoolName,
        mount_base: Option<&PathBuf>,
    ) -> Drone {
        let docker_config = DockerRuntimeConfig {
            runtime: None,
            log_config: None,
            mount_base: mount_base.map(|p| p.to_owned()),
            auto_prune: Some(false),
            cleanup_min_age: Some(Duration::zero()),
        };

        #[allow(deprecated)] // `docker_config` field is deprecated.
        let drone_config = DroneConfig {
            name: DroneName::new_random(),
            cluster: TEST_CLUSTER.parse().unwrap(),
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            db_path: Some(self.scratch_dir.join("drone.db")),
            pool: pool.clone(),
            auto_prune: None,
            cleanup_min_age: None,
            executor_config: Some(ExecutorConfig::Docker(docker_config)),
            docker_config: None,
            controller_url: controller.url().clone(),
        };

        Drone::run(drone_config).await.unwrap()
    }

    pub async fn drone(&mut self, controller: &ControllerServer) -> Drone {
        self.drone_internal(controller, &self.pool.clone(), None)
            .await
    }

    pub async fn drone_in_pool(
        &mut self,
        controller: &ControllerServer,
        pool: &DronePoolName,
    ) -> Drone {
        self.drone_internal(controller, pool, None).await
    }

    pub async fn drone_with_mount_base(
        &mut self,
        controller: &ControllerServer,
        mount_base: &PathBuf,
    ) -> Drone {
        self.drone_internal(controller, &self.pool.clone(), Some(mount_base))
            .await
    }

    pub async fn drone_with_socket(&mut self, controller: &ControllerServer) -> DroneWithSocket {
        let socket_path = self.scratch_dir.join("plane.sock");

        let executor_config = ExecutorConfig::UnixSocket(UnixSocketRuntimeConfig {
            socket_path: socket_path.clone(),
        });

        #[allow(deprecated)] // `docker_config` field is deprecated.
        let drone_config = DroneConfig {
            name: DroneName::new_random(),
            cluster: TEST_CLUSTER.parse().unwrap(),
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            db_path: Some(self.scratch_dir.join("drone.db")),
            pool: self.pool.clone(),
            auto_prune: None,
            cleanup_min_age: None,
            executor_config: Some(executor_config),
            docker_config: None,
            controller_url: controller.url().clone(),
        };

        let drone = Drone::run(drone_config).await.unwrap();

        DroneWithSocket::new(socket_path, drone).await
    }

    pub async fn dns(&mut self, controller: &ControllerServer) -> DnsServer {
        let client = controller.client();
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let name = AcmeDnsServerName::new_random();
        let handle = tokio::spawn(async move {
            run_dns_with_listener(name, client, listener, None)
                .await
                .unwrap();
        });

        DnsServer {
            port,
            _handle: Some(handle),
        }
    }

    pub async fn pebble(&mut self, dns_port: u16) -> Arc<Pebble> {
        let pebble = Arc::new(Pebble::new(self, dns_port, None).await.unwrap());
        self.drop_futures.lock().unwrap().push(pebble.clone());
        pebble
    }

    pub async fn pebble_with_eab(
        &mut self,
        dns_port: u16,
        eab_keypair: AcmeEabConfiguration,
    ) -> Arc<Pebble> {
        let pebble = Arc::new(
            Pebble::new(self, dns_port, Some(eab_keypair))
                .await
                .unwrap(),
        );
        self.drop_futures.lock().unwrap().push(pebble.clone());
        pebble
    }
}

#[allow(dead_code)] // Used in tests.
pub struct DroneWithSocket {
    pub socket_server: TypedUnixSocketServer<MessageToServer, MessageToClient>,
    pub drone: Drone,
    pub message_receiver: Receiver<MessageToServer>,
    pub request_receiver: Receiver<WrappedMessage<MessageToServer>>,
}

#[allow(dead_code)] // Used in tests.
impl DroneWithSocket {
    async fn new(socket_path: PathBuf, drone: Drone) -> Self {
        let socket_server =
            TypedUnixSocketServer::<MessageToServer, MessageToClient>::new(&socket_path)
                .await
                .unwrap();

        let message_receiver = socket_server.subscribe_events();
        let request_receiver = socket_server.subscribe_requests();

        Self {
            socket_server,
            drone,
            message_receiver,
            request_receiver,
        }
    }

    pub async fn receive_message(&mut self) -> MessageToServer {
        self.message_receiver.recv().await.unwrap()
    }

    pub async fn receive_request(&mut self) -> WrappedMessage<MessageToServer> {
        self.request_receiver.recv().await.unwrap()
    }

    pub async fn send_response(
        &self,
        request: &WrappedMessage<MessageToServer>,
        response: MessageToClient,
    ) {
        self.socket_server
            .send_response(request, response)
            .await
            .unwrap();
    }

    pub async fn send_message(&self, message: MessageToClient) {
        self.socket_server.send_message(message).await.unwrap();
    }
}

pub struct DnsServer {
    #[allow(dead_code)] // Used in tests.
    pub port: u16,
    _handle: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for DnsServer {
    fn drop(&mut self) {
        self._handle.take().unwrap().abort();
    }
}

fn get_project_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("not in cargo project");
    let manifest_dir_path = Path::new(&manifest_dir);
    let project_root = manifest_dir_path
        .ancestors()
        .find(|e| e.join(".git").exists())
        .expect("not in git repo");
    project_root.to_path_buf()
}

fn create_scratch_dir(name: &str) -> PathBuf {
    let scratch_dir = {
        let mut dir = get_project_root();
        dir.push(format!("test-scratch/{}", name));
        dir
    };
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
