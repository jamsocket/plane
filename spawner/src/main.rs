use crate::logging::init_logging;
use clap::Parser;
use dashmap::DashMap;
use idle_pod_collector::IdlePodCollector;
use name_generator::NameGenerator;
use serde::Deserialize;
use server::serve;
use std::sync::{Arc, Mutex};

mod hashutil;
mod idle_pod_collector;
mod kubernetes;
mod logging;
mod name_generator;
mod pod_state;
mod server;

#[allow(unused)]
#[derive(Deserialize, Debug)]
pub struct ConnectionState {
    active_connections: u32,
    seconds_inactive: u32,
    listening: bool,
}

#[derive(Parser, Clone)]
pub struct SpawnerParams {
    /// The name of the application image to deploy.
    #[clap(long)]
    application_image: String,

    /// The container port on which the application runs.
    #[clap(long, default_value = "8080")]
    application_port: u16,

    /// The name of the image to deploy as a monitoring sidecar.
    #[clap(long)]
    sidecar_image: Option<String>,

    /// The container port on which the sidecar runs. Only used if
    /// sidecar_image is set.
    #[clap(long, default_value = "7070")]
    sidecar_port: u16,

    /// The prefix used for public-facing URLs. To construct a full URL, it is
    /// appended with the pod name, unless nginx_internal_path is provided, in
    /// which case it is appended with the key.
    #[clap(long)]
    base_url: String,

    /// Prefix used as the prefix for the X-Accel-Redirect header for the
    /// /nginx_redirect endpoint. If set, the key (rather than the name)
    /// is used when constructing URLs.
    #[clap(long)]
    nginx_internal_path: Option<String>,

    /// The scheme used for key generation when nodes are initialized without
    /// a key. Defaults to UUID, other options look like "short:alphanum:5"
    /// (see documentation).
    #[clap(long)]
    name_generator: Option<String>,

    /// The namespace within which pods will be spawned.
    #[clap(long, default_value = "spawner")]
    namespace: String,

    /// How frequently (in seconds) to clean up idle containers.
    #[clap(long, default_value = "30")]
    cleanup_frequency_seconds: u32,
}

#[derive(Clone)]
pub struct SpawnerState {
    application_image: String,
    sidecar_image: Option<String>,
    base_url: String,

    application_port: u16,
    sidecar_port: u16,

    name_generator: Arc<Mutex<NameGenerator>>,
    key_map: Arc<DashMap<String, String>>,
    nginx_internal_path: Option<String>,
    namespace: String,
    cleanup_frequency_seconds: u32,
}

impl SpawnerState {
    pub fn url_for(&self, key: &str, name: &str) -> String {
        if self.nginx_internal_path.is_some() {
            format!("{}/{}/", self.base_url, key)
        } else {
            format!("{}/{}/", self.base_url, name)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), kube::Error> {
    let settings = SpawnerParams::parse();

    let name_generator = settings
        .name_generator
        .map(|ng| NameGenerator::from_str(&ng).expect("Could not parse name generator."))
        .unwrap_or_default();

    let state = SpawnerState {
        application_image: settings.application_image,
        application_port: settings.application_port,
        sidecar_image: settings.sidecar_image,
        sidecar_port: settings.sidecar_port,
        base_url: settings.base_url,
        name_generator: Arc::new(Mutex::new(name_generator)),
        key_map: Arc::new(DashMap::default()),
        nginx_internal_path: settings.nginx_internal_path,
        namespace: settings.namespace.clone(),
        cleanup_frequency_seconds: settings.cleanup_frequency_seconds,
    };

    let pod_collector = IdlePodCollector::new(state.clone());

    init_logging();

    tokio::select! {
        _ = pod_collector => tracing::warn!("controller drained"),
        _ = serve(state) => tracing::info!("server exited"),
    }
    Ok(())
}
