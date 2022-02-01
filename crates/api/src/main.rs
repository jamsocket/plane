use crate::logging::init_logging;
use axum::{extract::Extension, http::StatusCode, routing::post, AddExtensionLayer, Json, Router};
use clap::Parser;
use kube::{api::PostParams, Api, Client, ResourceExt};
use serde::{Deserialize, Serialize};
use spawner_resource::{SessionLivedBackend, SessionLivedBackendBuilder, SPAWNER_GROUP};
use std::{net::SocketAddr, sync::Arc};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

const DEFAULT_NAMESPACE: &str = "default";

mod logging;

#[derive(Parser)]
struct Opts {
    #[clap(long, default_value=DEFAULT_NAMESPACE)]
    namespace: String,

    #[clap(long, default_value = "8080")]
    port: u16,

    #[clap(long)]
    url_template: Option<String>,

    #[clap(long, default_value = "spawner-")]
    service_prefix: String,
}

struct Settings {
    namespace: String,
    url_template: Option<String>,
    service_prefix: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let opts = Opts::parse();

    let settings = Settings {
        namespace: opts.namespace,
        url_template: opts.url_template,
        service_prefix: opts.service_prefix,
    };

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    let app = Router::new()
        .route("/init", post(init_handler))
        .layer(AddExtensionLayer::new(Arc::new(settings)))
        .layer(trace_layer);

    let addr = SocketAddr::from(([0, 0, 0, 0], opts.port));
    tracing::info!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

#[derive(Deserialize)]
struct InitPayload {
    image: String,
}

#[derive(Serialize)]
struct InitResult {
    url: Option<String>,
    name: String,
}

async fn init_handler(
    Json(payload): Json<InitPayload>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Result<Json<InitResult>, StatusCode> {
    let slab =
        SessionLivedBackendBuilder::new(&payload.image).build_prefixed(&settings.service_prefix);

    // TODO: use pool?
    let client = Client::try_default().await.map_err(|error| {
        tracing::error!(%error, "Error getting client");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let api = Api::<SessionLivedBackend>::namespaced(client, &settings.namespace);

    let result = api
        .create(
            &PostParams {
                field_manager: Some(SPAWNER_GROUP.to_string()),
                ..PostParams::default()
            },
            &slab,
        )
        .await
        .map_err(|error| {
            tracing::error!(%error, "Error creating SessionLivedBackend.");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let prefixed_name = result.name();
    let name = if let Some(name) = prefixed_name.strip_prefix(&settings.service_prefix) {
        name.to_string()
    } else {
        tracing::warn!("Couldn't strip prefix from name.");
        return Err(StatusCode::EXPECTATION_FAILED);
    };

    let url = settings
        .url_template
        .as_ref()
        .map(|d| d.replace("{}", &name));

    tracing::info!(?url, %name, "Created SessionLivedBackend.");

    Ok(Json(InitResult { url, name }))
}
