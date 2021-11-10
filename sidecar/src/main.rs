use axum::{extract::Extension, routing::get, AddExtensionLayer, Json, Router};
use connection_monitor::ConnectionState;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use crate::connection_monitor::ConnectionMonitor;
use clap::Parser;

mod connection_monitor;
mod parse_helpers;
mod parse_proc;

const TCP_FILE: &str = "/proc/net/tcp";

#[derive(Parser)]
struct Opts {
    /// The port to open an HTTP server on to serve metrics requests.
    #[clap(long, default_value="7070")]
    serve_port: u16,

    /// The TCP port to monitor connection activity on.
    #[clap(long, default_value="8080")]
    monitor_port: u16,

    /// The rate (in seconds) at which to check for activity.
    #[clap(long, default_value="10")]
    refresh_rate_seconds: u64,
}

#[tokio::main]
async fn main() {
    // Initialize logging.
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let Opts {
        serve_port, refresh_rate_seconds, monitor_port
    } = Opts::parse();

    log::info!(
        "Monitoring {:?} for connections on port {} every {} seconds.",
        TCP_FILE,
        monitor_port,
        refresh_rate_seconds
    );

    // Set up network monitor.
    let mut connection_monitor = ConnectionMonitor::new(monitor_port, PathBuf::from(TCP_FILE));
    let state = connection_monitor.state();

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(refresh_rate_seconds)).await;
            log::info!("Refreshing.");
            connection_monitor.refresh();
        }
    });

    // Serve.
    let app = Router::new()
        .route("/", get(info))
        .layer(AddExtensionLayer::new(state));

    let addr = SocketAddr::from(([0, 0, 0, 0], serve_port));
    log::info!("Listening on {}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

/// HTTP endpoint which serves the connection state as JSON.
async fn info(monitor: Extension<Arc<ConnectionMonitor>>) -> Json<ConnectionState> {
    let state = monitor.state();
    Json(state)
}
