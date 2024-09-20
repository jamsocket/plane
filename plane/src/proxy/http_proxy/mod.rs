use super::connection_monitor::ConnectionMonitorHandle;
use super::rewriter::RequestRewriterError;
use super::route_map::RouteMap;
use super::ForwardableRequestInfo;
use crate::names::BackendName;
use crate::proxy::cert_manager::CertWatcher;
use crate::proxy::rewriter::RequestRewriter;
use crate::SERVER_NAME;
use axum::http::uri::PathAndQuery;
use futures_util::Future;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::{
    body::{Body, Bytes},
    service::Service,
    Request, Response,
};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use proxy_service::ProxyState;
use std::convert::Infallible;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{atomic::AtomicBool, Arc};
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use url::Url;

pub mod proxy_service;

pub struct RequestHandler {
    state: Arc<ProxyState>,
    https_redirect: bool,
    remote_meta: ForwardableRequestInfo,
    root_redirect_url: Option<Url>,
}

impl RequestHandler {
    async fn handle_request(
        self: Arc<Self>,
        req: hyper::Request<ProxyBody>,
    ) -> Result<hyper::Response<ProxyBody>, Infallible> {
        let result = self.handle_request_inner(req).await;
        match result {
            Ok(response) => Ok(response),
            Err(err) => {
                let (status_code, body) = match err {
                    ProxyError::InvalidConnectionToken => (
                        hyper::StatusCode::GONE,
                        "The backend is no longer available or the connection token is invalid.",
                    ),
                    ProxyError::MissingHostHeader => {
                        (hyper::StatusCode::BAD_REQUEST, "Bad request")
                    }
                    ProxyError::InvalidSubdomain => {
                        (hyper::StatusCode::UNAUTHORIZED, "Invalid subdomain")
                    }
                    ProxyError::BadRequest => (hyper::StatusCode::BAD_REQUEST, "Bad request"),
                    ProxyError::RequestError(err, backend) => {
                        tracing::warn!(?err, %backend, "Error proxying request to backend.");
                        (hyper::StatusCode::BAD_GATEWAY, "Connect error")
                    }
                    err => {
                        tracing::error!(?err, "Unhandled error handling request.");
                        (hyper::StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
                    }
                };
                let body: ProxyBody = to_proxy_body(body.to_string());
                Ok(response_builder()
                    .status(status_code)
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body(body)
                    .expect("Static response is always valid"))
            }
        }
    }

    async fn handle_request_inner(
        self: Arc<Self>,
        req: hyper::Request<ProxyBody>,
    ) -> Result<hyper::Response<ProxyBody>, ProxyError> {
        // Handle "/ready"
        if req.uri().path() == "/ready" {
            if self.state.connected() {
                return Ok(response_builder()
                    .status(hyper::StatusCode::OK)
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body(to_proxy_body("Plane Proxy server (ready)".to_string()))?);
            } else {
                return Ok(response_builder()
                    .status(hyper::StatusCode::SERVICE_UNAVAILABLE)
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body(to_proxy_body("Plane Proxy server (not ready)".to_string()))?);
            }
        }

        if self.https_redirect {
            let Some(host) = req
                .headers()
                .get(hyper::header::HOST)
                .and_then(|value| value.to_str().ok())
            else {
                return Err(ProxyError::MissingHostHeader);
            };

            let host = match host.parse() {
                Ok(host) => host,
                Err(err) => {
                    tracing::warn!(?err, ?host, "Invalid host header.");
                    return Err(ProxyError::BadRequest);
                }
            };

            let mut uri_parts = req.uri().clone().into_parts();
            uri_parts.scheme = Some("https".parse().expect("https is a valid scheme."));
            uri_parts.authority = Some(host);
            uri_parts.path_and_query = uri_parts
                .path_and_query
                .or_else(|| Some(PathAndQuery::from_static("")));
            let uri = hyper::Uri::from_parts(uri_parts).expect("URI parts are valid.");
            let result = response_builder()
                .status(hyper::StatusCode::MOVED_PERMANENTLY)
                .header(hyper::header::LOCATION, uri.to_string())
                .header(hyper::header::SERVER, SERVER_NAME)
                .body(empty_proxy_body());

            return Ok(result?);
        }

        if req.uri().path() == "/" {
            if let Some(root_redirect_url) = &self.root_redirect_url {
                return Ok(response_builder()
                    .status(hyper::StatusCode::MOVED_PERMANENTLY)
                    .header(hyper::header::LOCATION, root_redirect_url.to_string())
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body(empty_proxy_body())?);
            }
        }

        self.handle_proxy_request(req).await
    }

    async fn handle_proxy_request(
        self: Arc<Self>,
        req: hyper::Request<ProxyBody>,
    ) -> Result<hyper::Response<ProxyBody>, ProxyError> {
        let Some(mut request_rewriter) = RequestRewriter::new(req, self.remote_meta) else {
            tracing::warn!("Request rewriter failed to create.");
            return Err(ProxyError::BadRequest);
        };

        let route_info = self
            .state
            .route_map
            .lookup(request_rewriter.bearer_token())
            .await;

        let Some(route_info) = route_info else {
            return Err(ProxyError::InvalidConnectionToken);
        };

        let subdomain = match request_rewriter.get_subdomain(&route_info.cluster) {
            Ok(subdomain) => subdomain,
            Err(err) => {
                tracing::warn!(?err, "Subdomain not found in request rewriter.");
                return Err(ProxyError::InvalidSubdomain);
            }
        };
        if subdomain != route_info.subdomain.as_deref() {
            tracing::warn!(
                "Subdomain mismatch! subdomain in header: {:?}, subdomain in backend: {:?}",
                subdomain,
                route_info.subdomain
            );
            return Err(ProxyError::InvalidSubdomain);
        }

        let backend_id = route_info.backend_id.clone();
        request_rewriter.set_authority(route_info.address.0);

        let mut response = if request_rewriter.should_upgrade() {
            let (req, req_clone) = request_rewriter.into_request_pair(&route_info);

            let response = self
                .state
                .http_client
                .request(req_clone)
                .await
                .map_err(ProxyError::UpgradableRequestLegacyError)?;
            let response_clone = clone_response_empty_body(&response);

            let response_upgrade = hyper::upgrade::on(response)
                .await
                .map_err(ProxyError::UpgradeError)?;
            let monitor = self.state.monitor.monitor();
            let backend_id = backend_id.clone();

            tokio::spawn(async move {
                let req_upgrade = match hyper::upgrade::on(req).await {
                    Ok(req) => req,
                    Err(error) => {
                        tracing::error!(?error, "Error upgrading connection.");
                        return;
                    }
                };

                monitor
                    .lock()
                    .expect("Monitor lock was poisoned.")
                    .inc_connection(&backend_id);

                let mut response_upgrade = TokioIo::new(response_upgrade);

                let mut req_upgrade = TokioIo::new(req_upgrade);

                match copy_bidirectional(&mut req_upgrade, &mut response_upgrade).await {
                    Ok(_) => (),
                    Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
                        tracing::info!("Upgraded connection closed with UnexpectedEof.");
                    }
                    Err(error) if error.kind() == ErrorKind::TimedOut => {
                        tracing::info!("Upgraded connection timed out.");
                    }
                    Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                        tracing::info!("Connection reset by peer.");
                    }
                    Err(error) if error.kind() == ErrorKind::BrokenPipe => {
                        tracing::info!("Broken pipe.");
                    }
                    Err(error) => {
                        tracing::error!(?error, "Error with upgraded connection.");
                    }
                }

                monitor
                    .lock()
                    .expect("Monitor lock was poisoned.")
                    .dec_connection(&backend_id);
            });

            response_clone
        } else {
            let req = request_rewriter.into_request(&route_info);
            self.state.monitor.touch_backend(&backend_id);
            let response = self
                .state
                .http_client
                .request(req)
                .await
                .map_err(|e| ProxyError::RequestError(e, backend_id.clone()))?;

            box_response_body(response)
        };

        let headers = response.headers_mut();
        headers.insert(
            PLANE_BACKEND_ID_HEADER,
            backend_id
                .to_string()
                .parse()
                .expect("Backend ID is a valid header value."),
        );

        for (key, value) in DEFAULT_CORS_HEADERS {
            if !headers.contains_key(*key) {
                headers.insert(
                    *key,
                    value.parse().expect("CORS header is a valid header value."),
                );
            }
        }

        Ok(response)
    }
}
