use crate::config::HttpOptions;
use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use plane_core::{
    messages::{
        agent::{DockerExecutableConfig, DockerPullPolicy, ResourceLimits},
        scheduler::{ScheduleRequest, ScheduleResponse},
    },
    nats::TypedNats,
    types::{ClusterName, ResourceLock},
    Never,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

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
    pub fn into_spawn_request(
        &self,
        cluster: ClusterName,
        image: String,
    ) -> Result<ScheduleRequest> {
        let max_idle_secs = Duration::from_secs(self.grace_period_seconds.unwrap_or_else(|| 600));

        let lock: Option<ResourceLock> = self.lock.clone().map(|l| l.try_into()).transpose()?;

        let executable = DockerExecutableConfig {
            image,
            env: self.env.clone(),
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
    ready_url: String,
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
    Path(service): Path<String>,
    Json(request): Json<HttpSpawnRequest>,
) -> Result<Json<HttpSpawnResponse>, StatusCode> {
    let service = server_state
        .services
        .get(&service)
        .ok_or_else(|| StatusCode::NOT_FOUND)?;

    let request = request
        .into_spawn_request(server_state.cluster.clone(), service.clone())
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
            url: format!(
                "http://{}.{}",
                backend_id.to_string(),
                server_state.cluster.to_string(),
            ),
            name: backend_id.to_string(),
            ready_url: format!("__todo"),
            status_url: format!("__todo"),
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

pub async fn serve(options: HttpOptions, nats: TypedNats) -> Result<Never> {
    let addr = SocketAddr::new(options.bind_ip, options.port);

    let server_state = Arc::new(ServerState {
        nats,
        services: options.services,
        cluster: options.cluster,
    });

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/service/:service/spawn", post(handle_spawn))
        .with_state(server_state);

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Err(anyhow::anyhow!("HTTP server exited unexpectedly"))
}
