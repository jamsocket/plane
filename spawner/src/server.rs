use crate::kubernetes::create_pod;
use crate::logging::LogError;
use crate::state::SpawnerState;
use axum::extract::Query;
use axum::{
    extract::Extension,
    http::StatusCode,
    routing::{get, post},
    AddExtensionLayer, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;

async fn status() -> Json<Value> {
    Json(json!({
        "status": "ok"
    }))
}

#[derive(Deserialize)]
struct InitQuery {
    account: Option<String>,
}

#[derive(Serialize)]
struct PodResult {
    pod: String,
    url: String,
}

async fn init(
    Extension(spawner_state): Extension<SpawnerState>,
    Query(InitQuery { account }): Query<InitQuery>,
) -> Result<Json<PodResult>, StatusCode> {
    let pod_id = create_pod(account.as_deref(), &spawner_state.settings)
        .await
        .log_error_internal()?;

    let pod_url = spawner_state.settings.url_for(&pod_id);

    tracing::info!(?pod_id, %pod_url, "Created pod.");

    Ok(Json(PodResult {
        pod: pod_id.name(),
        url: pod_url,
    }))
}

pub async fn serve(state: SpawnerState) -> Result<(), kube::Error> {
    let app = Router::new()
        .route("/", get(status))
        .route("/init", post(init))
        .layer(AddExtensionLayer::new(state));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));

    tracing::info!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();

    Ok(())
}
