use crate::hashutil::hash_key;
use crate::logging::LogError;
use crate::{kubernetes::create_pod, SpawnerState};
use axum::body::Body;
use axum::extract::Query;
use axum::http::header::HeaderName;
use axum::http::{HeaderValue, Request, Response};
use axum::{
    extract::Extension,
    http::StatusCode,
    routing::{any, get, post},
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
    key: Option<String>,
    account: Option<String>,
}

#[derive(Serialize)]
struct PodResult {
    pod: String,
    key: String,
    url: String,
}

async fn init(
    Extension(spawner_settings): Extension<SpawnerState>,
    Query(InitQuery { key, account }): Query<InitQuery>,
) -> Result<Json<PodResult>, StatusCode> {
    let key = if let Some(key) = key {
        key
    } else {
        spawner_settings
            .name_generator
            .lock()
            .log_error_internal()?
            .generate()
    };

    if spawner_settings.key_map.contains_key(&key) {
        tracing::warn!(%key, "Tried to create pod, but key already exists.");

        // TODO: check if the pod still exists.
        return Err(StatusCode::CONFLICT);
    }

    let pod_name = hash_key(&key);
    let pod_url = spawner_settings.url_for(&key, &pod_name);

    tracing::info!(%key, %pod_name, %pod_url, "Creating pod.");

    create_pod(
        &key,
        &pod_name,
        &pod_url,
        account.as_deref(),
        &spawner_settings,
    )
    .await
    .log_error_internal()?;

    spawner_settings
        .key_map
        .insert(key.clone(), pod_name.clone());

    Ok(Json(PodResult {
        pod: pod_name,
        key,
        url: pod_url,
    }))
}

#[derive(Deserialize)]
struct GetQuery {
    key: String,
}

async fn get_name(
    Extension(spawner_settings): Extension<SpawnerState>,
    Query(GetQuery { key }): Query<GetQuery>,
) -> Result<Json<PodResult>, StatusCode> {
    if let Some(pod_name) = spawner_settings.key_map.get(&key) {
        Ok(Json(PodResult {
            pod: pod_name.to_string(),
            url: spawner_settings.url_for(&key, &pod_name),
            key,
        }))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn nginx_redirect(
    Extension(spawner_settings): Extension<SpawnerState>,
    mut request: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let nginx_internal_path =
        if let Some(nginx_internal_path) = spawner_settings.nginx_internal_path {
            nginx_internal_path
        } else {
            return Err(StatusCode::MISDIRECTED_REQUEST);
        };

    let uri = request.uri_mut();
    let path_and_query = uri.path_and_query().unwrap();
    let path = path_and_query
        .path()
        .strip_prefix("/nginx_redirect/")
        .unwrap();
    let (key, path) = path.split_once('/').unwrap_or((path, ""));
    let query_string = if let Some(query) = path_and_query.query() {
        format!("?{}", query)
    } else {
        "".to_string()
    };

    if let Some(pod_name) = spawner_settings.key_map.get(key) {
        let url = format!(
            "{}/{}/{}{}",
            nginx_internal_path, *pod_name, path, query_string
        );

        let response = Response::builder()
            .header(
                HeaderName::from_static("x-accel-redirect"),
                HeaderValue::from_str(&url).unwrap(),
            )
            .body(Body::empty())
            .log_error_internal()?;
        Ok(response)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn redirect(
    Extension(spawner_settings): Extension<SpawnerState>,
    mut request: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let uri = request.uri_mut();
    let path_and_query = uri.path_and_query().unwrap();
    let path = path_and_query.path().strip_prefix("/redirect/").unwrap();
    let (key, path) = path.split_once('/').unwrap_or((path, ""));
    let query_string = if let Some(query) = path_and_query.query() {
        format!("?{}", query)
    } else {
        "".to_string()
    };

    if let Some(pod_name) = spawner_settings.key_map.get(key) {
        let url = format!(
            "{}/{}/{}{}",
            spawner_settings.base_url, *pod_name, path, query_string
        );

        let response = Response::builder()
            .header(
                HeaderName::from_static("location"),
                HeaderValue::from_str(&url).unwrap(),
            )
            .body(Body::empty())
            .log_error_internal()?;
        Ok(response)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn serve(state: SpawnerState) -> Result<(), kube::Error> {
    let app = Router::new()
        .route("/", get(status))
        .route("/init", post(init))
        .route("/get", get(get_name))
        .route("/nginx_redirect/*path", any(nginx_redirect))
        .route("/redirect/*path", any(redirect))
        .layer(AddExtensionLayer::new(state));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));

    tracing::info!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();

    Ok(())
}
