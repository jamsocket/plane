use crate::logging::init_logging;
use dis_spawner::api::{backend_routes, spawn_handler, ApiSettings};
use axum::{extract::Extension, routing::post, Router};
use clap::Parser;
use kube::Client;
use std::{net::SocketAddr, sync::Arc};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

mod logging;

#[derive(Parser)]
struct Opts {
    /// Kubernetes namespace within which SessionLivedBackend instances will be spawned.
    /// Must already exist.
    #[clap(short, long, default_value = "spawner")]
    namespace: String,

    /// Port on which the API server runs.
    #[clap(short, long, default_value = "8080")]
    port: u16,

    /// Template for the URL of spawned backends. The substring {} will be replaced by
    /// the backend's ID, which is the pod name with the service prefix removed.
    /// If this is not provided, the backend URL will not be generated when a backend is
    /// spawned.
    #[clap(long)]
    url_template: Option<String>,

    /// The base URL for the API server. If this is not provided, the "status" and "ready"
    /// URLs will not be returned when a backend is spawned.
    #[clap(long)]
    api_server_base: Option<String>,

    /// The prefix to use as the name of objects associated with a backend. These objects
    /// include the SessionLivedBackend, Pod, and Service.
    #[clap(long, default_value = "spawner-")]
    service_prefix: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let opts = Opts::parse();

    let client = Client::try_default().await?;

    let settings = ApiSettings {
        namespace: opts.namespace,
        url_template: opts.url_template,
        service_prefix: opts.service_prefix,
        api_server_base: opts.api_server_base,
        client,
    };

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    let app = Router::new()
        .route("/spawn", post(spawn_handler))
        .nest("/backend", backend_routes())
        .layer(Extension(Arc::new(settings)))
        .layer(trace_layer);

    let addr = SocketAddr::from(([0, 0, 0, 0], opts.port));
    tracing::debug!(%addr, "Listening");
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
