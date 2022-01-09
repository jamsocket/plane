use crate::{
    logging::init_logging,
    state::{SpawnerSettings, SpawnerState},
};
use clap::Parser;
use idle_pod_collector::IdlePodCollector;
use serde::Deserialize;
use server::serve;

mod idle_pod_collector;
mod kubernetes;
mod logging;
mod pod_id;
mod pod_state;
mod server;
mod state;

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

    /// The namespace within which pods will be spawned.
    #[clap(long, default_value = "spawner")]
    namespace: String,

    /// How frequently (in seconds) to clean up idle containers.
    #[clap(long, default_value = "30")]
    cleanup_frequency_seconds: u32,
}

#[tokio::main]
async fn main() -> Result<(), kube::Error> {
    let settings = SpawnerParams::parse();

    let settings = SpawnerSettings {
        application_image: settings.application_image,
        application_port: settings.application_port,
        sidecar_image: settings.sidecar_image,
        sidecar_port: settings.sidecar_port,
        base_url: settings.base_url,
        namespace: settings.namespace.clone(),
        cleanup_frequency_seconds: settings.cleanup_frequency_seconds,
    };

    let (_pod_collector, drainer, store) = IdlePodCollector::new(settings.clone()).await;

    let state = SpawnerState {
        settings: settings,
        store,
    };

    init_logging();

    tokio::select! {
        _ = drainer => tracing::warn!("controller drained"),
        _ = serve(state) => tracing::info!("server exited"),
    }
    Ok(())
}
