use crate::logging::init_logging;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::{sse::Event as AxumSseEvent, Sse},
    routing::{get, post},
    AddExtensionLayer, BoxError, Json, Router,
};
use clap::Parser;
use futures::{Stream, StreamExt, TryStreamExt};
use k8s_openapi::{api::core::v1::Event as KubeEventResource, serde_json::json};
use kube::{
    api::{ListParams, PostParams},
    runtime::watcher::{watcher, Error as KubeWatcherError, Event as KubeWatcherEvent},
    Api, Client, ResourceExt,
};
use serde::{Deserialize, Serialize};
use spawner_resource::{SessionLivedBackend, SessionLivedBackendBuilder, SPAWNER_GROUP};
use std::{future::ready, net::SocketAddr, sync::Arc};
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
        .route("/:backend_id/status", get(status_handler))
        .layer(AddExtensionLayer::new(Arc::new(settings)))
        .layer(trace_layer);

    let addr = SocketAddr::from(([0, 0, 0, 0], opts.port));
    tracing::info!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

fn get_event_stream(
    client: Client,
    resource_name: &str,
    namespace: &str,
) -> impl Stream<Item = Result<KubeWatcherEvent<KubeEventResource>, KubeWatcherError>> {
    let api = Api::<KubeEventResource>::namespaced(client, &namespace);

    let list_params = ListParams {
        field_selector: Some(format!("involvedObject.name={}", resource_name)),
        ..ListParams::default()
    };

    watcher(api, list_params)
}

fn convert_stream<T>(stream: T) -> impl Stream<Item = Result<AxumSseEvent, BoxError>>
where
    T: Stream<Item = Result<KubeWatcherEvent<KubeEventResource>, KubeWatcherError>>,
{
    stream.filter_map(|event| {
        match event {
            Ok(event) => match event {
                KubeWatcherEvent::Applied(event) => {
                    if let Some(state) = event.action {
                        ready(Some(
                            AxumSseEvent::default()
                                .json_data(json! ({
                                    "state": state,
                                    "time": event.event_time,
                                }))
                                .map_err(|e| e.into()),
                        ))
                    } else {
                        ready(None)
                    }
                },
                _ => ready(None)
            },
            Err(err) => ready(Some(Err(err.into())))
        }
    })
}

async fn status_handler(
    Path((backend_id,)): Path<(String,)>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Result<Sse<impl Stream<Item = Result<AxumSseEvent, BoxError>>>, StatusCode> {
    let client = Client::try_default().await.map_err(|error| {
        tracing::error!(%error, "Error getting client");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let events = get_event_stream(
        client,
        &format!("{}{}", settings.service_prefix, backend_id),
        &settings.namespace,
    );

    let sse_events: _ = convert_stream(events).into_stream();

    Ok(Sse::new(sse_events))
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
