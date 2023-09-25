use crate::config::HttpOptions;
use anyhow::Result;
use async_stream::stream;
use axum::{
    extract::{Path, State},
    headers::Host,
    http::{header, Method, StatusCode},
    response::{sse::Event, Sse},
    routing::{get, post},
    BoxError, Json, Router, TypedHeader,
};
use chrono::{DateTime, Utc};
use futures::{Stream, TryStreamExt};
use plane_core::{
    messages::{
        agent::{
            BackendState, BackendStateMessage, DockerExecutableConfig, DockerPullPolicy,
            ResourceLimits,
        },
        scheduler::{ScheduleRequest, ScheduleResponse},
    },
    nats::TypedNats,
    types::{BackendId, ClusterName, ResourceLock},
    Never,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tower_http::cors::{Any, CorsLayer};

const DEFAULT_GRACE_PERIOD_SECONDS: u64 = 60;

trait LogError<T> {
    fn log_error(self, status_code: StatusCode) -> Result<T, StatusCode>;
}

impl<T> LogError<T> for Result<T> {
    fn log_error(self, status_code: StatusCode) -> Result<T, StatusCode> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::error!(error = %e, "HTTP request failed.");
                Err(status_code)
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct HttpSpawnRequest {
    grace_period_seconds: Option<u64>,
    lock: Option<String>,
    #[serde(default = "bool::default")]
    require_bearer_token: bool,
    port: Option<u16>,
    #[serde(default = "HashMap::default")]
    env: HashMap<String, String>,
    #[serde(default = "Vec::default")]
    volume_mounts: Vec<Value>,
}

impl HttpSpawnRequest {
    pub fn as_spawn_request(&self, cluster: ClusterName, image: String) -> Result<ScheduleRequest> {
        let max_idle_secs = Duration::from_secs(
            self.grace_period_seconds
                .unwrap_or(DEFAULT_GRACE_PERIOD_SECONDS),
        );

        let lock: Option<ResourceLock> = self.lock.clone().map(|l| l.try_into()).transpose()?;

        let mut env = self.env.clone();
        env.insert("PORT".to_string(), self.port.unwrap_or(8080).to_string());

        let executable = DockerExecutableConfig {
            image,
            env,
            credentials: None,
            resource_limits: ResourceLimits::default(),
            pull_policy: DockerPullPolicy::default(),
            port: self.port,
            volume_mounts: self.volume_mounts.clone(),
        };

        Ok(ScheduleRequest {
            cluster,
            max_idle_secs,
            lock,
            require_bearer_token: self.require_bearer_token,
            backend_id: None,
            metadata: HashMap::new(),
            executable,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct HttpSpawnResponse {
    url: String,
    name: String,
    // TODO: implement ready URL for jamsocket compatibility?
    // ready_url: String,
    status_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bearer_token: Option<String>,
    spawned: bool,
}

async fn handle_index() -> &'static str {
    "Plane controller HTTP server"
}

async fn handle_spawn(
    State(server_state): State<Arc<ServerState>>,
    TypedHeader(host): TypedHeader<Host>,
    Path(service): Path<String>,
    Json(request): Json<HttpSpawnRequest>,
) -> Result<Json<HttpSpawnResponse>, StatusCode> {
    let service = server_state
        .services
        .get(&service)
        .ok_or(StatusCode::NOT_FOUND)?;

    let request = request
        .as_spawn_request(server_state.cluster.clone(), service.clone())
        .log_error(StatusCode::BAD_REQUEST)?;

    let response = server_state
        .nats
        .request(&request)
        .await
        .log_error(StatusCode::INTERNAL_SERVER_ERROR)?;

    let http_response = match response {
        ScheduleResponse::Scheduled {
            backend_id,
            bearer_token,
            spawned,
            ..
        } => HttpSpawnResponse {
            url: format!("http://{}.{}", backend_id, server_state.cluster,),
            name: backend_id.to_string(),
            status_url: format!("http://{}/backend/{}/status", host, backend_id),
            bearer_token,
            spawned,
        },
        ScheduleResponse::NoDroneAvailable => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    Ok(Json(http_response))
}

struct ServerState {
    pub nats: TypedNats,
    pub services: HashMap<String, String>,
    pub cluster: ClusterName,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ApiBackendStateMessage {
    /// The new state.
    pub state: BackendState,

    /// The backend id.
    pub backend: BackendId,

    /// The time the state change was observed.
    pub time: DateTime<Utc>,
}

fn convert_msg<T: Serialize>(msg: &T) -> Result<Event, BoxError> {
    Event::default().json_data(msg).map_err(|e| e.into())
}

async fn status_stream(
    Path((backend_name,)): Path<(String,)>,
    State(server_state): State<Arc<ServerState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, BoxError>>>, StatusCode> {
    let mut events = server_state
        .nats
        .subscribe_jetstream_subject(BackendStateMessage::subscribe_subject(&BackendId::new(
            backend_name,
        )))
        .await
        .log_error(StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut prev_state = None;

    let dedup_messages = stream! {
        while let Some((msg, _)) = events.next().await {
            if Some(msg.state) != prev_state {
                let msg = ApiBackendStateMessage {
                    state: msg.state,
                    backend: msg.backend,
                    time: msg.time,
                };

                yield convert_msg(&msg);
                prev_state = Some(msg.state);
            };
        };
    };

    let sse_events = dedup_messages.into_stream();

    Ok(Sse::new(sse_events))
}

async fn status(
    Path((backend_name,)): Path<(String,)>,
    State(server_state): State<Arc<ServerState>>,
) -> Result<Json<ApiBackendStateMessage>, StatusCode> {
    let mut events = server_state
        .nats
        .subscribe_jetstream_subject(BackendStateMessage::subscribe_subject(&BackendId::new(
            backend_name,
        )))
        .await
        .log_error(StatusCode::INTERNAL_SERVER_ERROR)?;

    while let Some((msg, meta)) = events.next().await {
        if meta.pending > 0 {
            continue;
        }

        let msg = ApiBackendStateMessage {
            state: msg.state,
            backend: msg.backend,
            time: msg.time,
        };

        return Ok(Json(msg));
    }

    Err(StatusCode::NOT_FOUND)
}

pub async fn serve(options: HttpOptions, nats: TypedNats) -> Result<Never> {
    let addr = SocketAddr::new(options.bind_ip, options.port);

    let server_state = Arc::new(ServerState {
        nats,
        services: options.services,
        cluster: options.cluster,
    });

    let cors_public = CorsLayer::new()
        .allow_methods(vec![Method::GET, Method::POST])
        .allow_headers(vec![header::CONTENT_TYPE])
        .allow_origin(Any);

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/service/:service/spawn", post(handle_spawn))
        .route(
            "/backend/:backend_id/status",
            get(status).layer(cors_public.clone()),
        )
        .route(
            "/backend/:backend_id/status/stream",
            get(status_stream).layer(cors_public),
        )
        .with_state(server_state);

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Err(anyhow::anyhow!("HTTP server exited unexpectedly"))
}
