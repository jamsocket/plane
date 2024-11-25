use self::{
    backend_state::{handle_backend_status, handle_backend_status_stream},
    cluster_state::handle_cluster_state,
    connect::handle_revoke,
    dns::handle_dns_socket,
    drain::handle_drain,
    error::IntoApiError,
    proxy::handle_proxy_socket,
};
use crate::{
    cleanup,
    controller::{connect::handle_connect, core::Controller, drone::handle_drone_socket},
    database::{connect_and_migrate, PlaneDatabase},
    heartbeat_consts::HEARTBEAT_INTERVAL,
    signals::wait_for_shutdown_signal,
    util::GuardHandle,
};
use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header, Method},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use forward_auth::forward_layer;
use futures_util::never::Never;
use plane_client::{
    names::ControllerName,
    protocol::StatusResponse,
    types::ClusterName,
    version::{PLANE_GIT_HASH, PLANE_VERSION},
    PlaneClient,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::{
    net::TcpListener,
    sync::oneshot::{self},
    task::JoinHandle,
};
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::DefaultOnFailure,
};
use tracing::Level;
use url::Url;

mod backend_state;
mod cluster_state;
pub mod command;
mod connect;
mod core;
mod dns;
mod drain;
mod drone;
pub mod error;
mod forward_auth;
mod proxy;
mod terminate;

/// How long to wait for the server to terminate gracefully before forcing it to shut down.
/// We want to keep this just high enough to serve short requests. Long-lived requests
/// will continue blocking indefinitely, which is why we need a timeout.
const TERMINATE_TIMEOUT_DURATION: std::time::Duration = std::time::Duration::from_secs(2);

pub async fn status(
    State(controller): State<Controller>,
) -> Result<Json<StatusResponse>, Response> {
    controller
        .db
        .health_check()
        .await
        .or_internal_error("Database health check failed")?;

    Ok(Json(StatusResponse {
        status: "ok".to_string(),
        version: PLANE_VERSION.to_string(),
        hash: PLANE_GIT_HASH.to_string(),
    }))
}

pub async fn health(State(controller): State<Controller>) -> Result<Json<Value>, Response> {
    controller
        .db
        .health_check()
        .await
        .or_internal_error("Database health check failed")?;

    Ok(Json(json!({
        "status": "ok"
    })))
}

struct HeartbeatSender {
    handle: JoinHandle<Never>,
    db: PlaneDatabase,
    controller_id: ControllerName,
}

impl HeartbeatSender {
    pub async fn start(db: PlaneDatabase, controller_id: ControllerName) -> Result<Self> {
        // Wait until we have sent the initial heartbeat.
        db.controller().heartbeat(&controller_id, true).await?;

        let db_clone = db.clone();
        let controller_id_clone = controller_id.clone();
        let handle: JoinHandle<Never> = tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                if let Err(err) = db_clone
                    .controller()
                    .heartbeat(&controller_id_clone, true)
                    .await
                {
                    tracing::error!(?err, "Failed to send heartbeat");
                }
            }
        });

        Ok(Self {
            handle,
            db,
            controller_id,
        })
    }

    pub async fn terminate(&self) {
        self.handle.abort();
        if let Err(err) = self
            .db
            .controller()
            .heartbeat(&self.controller_id, false)
            .await
        {
            tracing::error!(?err, "Failed to send offline heartbeat");
        }
    }
}

pub struct ControllerServer {
    bind_addr: SocketAddr,
    controller_id: ControllerName,
    graceful_terminate_sender: Option<oneshot::Sender<()>>,
    heartbeat_handle: HeartbeatSender,
    // server_handle is wrapped in an Option<> because we need to take ownership of it to join it
    // when gracefully terminating.
    server_handle: Option<JoinHandle<Result<(), std::io::Error>>>,
    _cleanup_handle: GuardHandle,
}

impl ControllerServer {
    pub async fn run(config: ControllerConfig) -> Result<Self> {
        let listener = TcpListener::bind(config.bind_addr).await?;

        tracing::info!("Attempting to connect to database...");

        let db = connect_and_migrate(&config.db_url)
            .await
            .context("Failed to connect to database and run migrations.")?;

        tracing::info!("Connected to database. Listening for connections.");

        Self::run_with_listener(
            db,
            listener,
            config.id,
            config.controller_url,
            config.default_cluster,
            config.cleanup_min_age_days,
            config.cleanup_batch_size,
            config.forward_auth,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_listener(
        db: PlaneDatabase,
        listener: TcpListener,
        id: ControllerName,
        controller_url: Url,
        default_cluster: Option<ClusterName>,
        cleanup_min_age_days: Option<i32>,
        cleanup_batch_size: Option<i32>,
        forward_auth: Option<Url>,
    ) -> Result<Self> {
        let bind_addr = listener.local_addr()?;

        let cleanup_handle = {
            let db = db.clone();
            GuardHandle::new(async move {
                cleanup::run_cleanup_loop(db.clone(), cleanup_min_age_days, cleanup_batch_size)
                    .await
            })
        };

        let (graceful_terminate_sender, graceful_terminate_receiver) =
            tokio::sync::oneshot::channel::<()>();

        let controller =
            Controller::new(db.clone(), id.clone(), controller_url, default_cluster).await;

        let trace_layer = TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_request(DefaultOnRequest::new().level(Level::DEBUG))
            .on_failure(DefaultOnFailure::new().level(Level::WARN))
            .on_response(DefaultOnResponse::new().level(Level::DEBUG));

        let heartbeat_handle = HeartbeatSender::start(db.clone(), id.clone()).await?;

        // Routes that relate to controlling the system (spawning and terminating drones)
        // or that otherwise expose non-public system information.
        //
        // These routes should not be exposed on the open internet without an authorization
        // barrier (such as a reverse proxy) in front.
        let mut control_routes = Router::new()
            .route("/status", get(status))
            .route("/c/:cluster/state", get(handle_cluster_state))
            .route("/c/:cluster/drone-socket", get(handle_drone_socket))
            .route("/c/:cluster/proxy-socket", get(handle_proxy_socket))
            .route("/dns-socket", get(handle_dns_socket))
            .route("/connect", post(handle_connect))
            .route("/c/:cluster/d/:drone/drain", post(handle_drain))
            .route(
                "/b/:backend/soft-terminate",
                post(terminate::handle_soft_terminate),
            )
            .route(
                "/b/:backend/hard-terminate",
                post(terminate::handle_hard_terminate),
            )
            .route(
                "/b/revoke",
                post(handle_revoke), // (TODO) does not notify proxies, see handler function for details
            );

        if let Some(forward_auth_url) = forward_auth {
            tracing::info!(?forward_auth_url, "Forward auth enabled");
            let forward_url = forward_auth_url.clone();

            control_routes = control_routes.layer(axum::middleware::from_fn_with_state(
                forward_url.clone(),
                forward_layer,
            ));
        }

        let cors_public = CorsLayer::new()
            .allow_methods(vec![Method::GET, Method::POST])
            .allow_headers(vec![header::CONTENT_TYPE])
            .allow_origin(Any);

        // Routes that are may be accessed directly from end-user code. These are placed
        // under the /pub/ top-level route to make it easier to expose only these routes,
        // using a reverse proxy configuration.
        let public_routes = Router::new()
            .route("/b/:backend/status", get(handle_backend_status))
            .route(
                "/b/:backend/status-stream",
                get(handle_backend_status_stream),
            )
            .route("/health", get(health))
            .layer(cors_public.clone());

        let app = Router::new()
            .nest("/pub", public_routes)
            .nest("/ctrl", control_routes)
            .layer(trace_layer)
            .with_state(controller);

        let server_handle = tokio::spawn(async {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                graceful_terminate_receiver.await.ok();
            })
            .await
        });

        Ok(Self {
            graceful_terminate_sender: Some(graceful_terminate_sender),
            heartbeat_handle,
            server_handle: Some(server_handle),
            controller_id: id,
            bind_addr,
            _cleanup_handle: cleanup_handle,
        })
    }

    pub async fn terminate(&mut self) {
        // Stop sending online heartbeat.
        self.heartbeat_handle.terminate().await;

        // Begin graceful shutdown of server.
        tracing::info!("Initiating graceful shutdown of server");
        let Some(graceful_terminate_sender) = self.graceful_terminate_sender.take() else {
            return;
        };

        if let Err(err) = graceful_terminate_sender.send(()) {
            tracing::error!(?err, "Failed to send graceful terminate signal");
        } else {
            // Wait for server to finish shutting down.
            let Some(server_handle) = self.server_handle.take() else {
                return;
            };

            match tokio::time::timeout(TERMINATE_TIMEOUT_DURATION, server_handle).await {
                Ok(Ok(Ok(()))) => {
                    tracing::info!("Server gracefully terminated");
                }
                Ok(Ok(Err(err))) => {
                    tracing::error!(?err, "Server error");
                }
                Ok(Err(err)) => {
                    tracing::error!(?err, "Server error");
                }
                Err(_) => {
                    tracing::warn!("Server did not terminate gracefully in time, forcing shutdown");
                }
            }
        }
    }

    pub fn id(&self) -> &ControllerName {
        &self.controller_id
    }

    pub fn url(&self) -> Url {
        let base_url: Url = format!("http://{}", self.bind_addr)
            .parse()
            .expect("Generated URI is always valid.");
        base_url
    }

    pub fn client(&self) -> PlaneClient {
        let base_url: Url = self.url();
        PlaneClient::new(base_url)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerConfig {
    pub db_url: String,
    pub bind_addr: SocketAddr,
    pub id: ControllerName,
    pub controller_url: Url,
    pub default_cluster: Option<ClusterName>,
    pub cleanup_min_age_days: Option<i32>,
    pub cleanup_batch_size: Option<i32>,
    pub forward_auth: Option<Url>,
}

pub async fn run_controller(config: ControllerConfig) -> Result<()> {
    let mut server = ControllerServer::run(config).await?;

    wait_for_shutdown_signal().await;

    server.terminate().await;

    Ok(())
}
