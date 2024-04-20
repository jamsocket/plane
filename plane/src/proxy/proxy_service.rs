use super::connection_monitor::ConnectionMonitorHandle;
use super::rewriter::RequestRewriterError;
use super::route_map::RouteMap;
use super::tls::TlsStream;
use super::{ForwardableRequestInfo, Protocol};
use crate::proxy::cert_manager::CertWatcher;
use crate::proxy::rewriter::RequestRewriter;
use crate::proxy::tls::TlsAcceptor;
use crate::SERVER_NAME;
use axum::http::uri::PathAndQuery;
use futures_util::{Future, FutureExt};
use hyper::server::conn::AddrIncoming;
use hyper::{
    client::HttpConnector, server::conn::AddrStream, service::Service, Body, Request, Response,
};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{atomic::AtomicBool, Arc};
use std::{
    future::ready,
    io::ErrorKind,
    task::{self, Poll},
};
use tokio::io::copy_bidirectional;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::ServerConfig;
use url::Url;

const PLANE_BACKEND_ID_HEADER: &str = "x-plane-backend-id";

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Invalid or expired connection token")]
    InvalidConnectionToken,

    #[error("Missing `host` header")]
    MissingHostHeader,

    #[error("Bad request")]
    BadRequest,

    #[error("Invalid subdomain")]
    InvalidSubdomain,

    #[error("HTTP error: {0}")]
    HttpError(#[from] hyper::http::Error),

    #[error("Hyper error: {0}")]
    HyperError(#[from] hyper::Error),
}

impl From<RequestRewriterError> for ProxyError {
    fn from(err: RequestRewriterError) -> Self {
        match err {
            RequestRewriterError::InvalidHostHeader => ProxyError::BadRequest,
        }
    }
}

pub struct ProxyState {
    pub route_map: RouteMap,
    http_client: hyper::Client<HttpConnector>,
    pub monitor: ConnectionMonitorHandle,
    connected: AtomicBool,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyState {
    pub fn new() -> Self {
        Self {
            route_map: RouteMap::new(),
            http_client: hyper::Client::builder().build_http::<hyper::Body>(),
            monitor: ConnectionMonitorHandle::new(),
            connected: AtomicBool::new(false),
        }
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected
            .store(connected, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }
}

struct RequestHandler {
    state: Arc<ProxyState>,
    https_redirect: bool,
    remote_meta: ForwardableRequestInfo,
    root_redirect_url: Option<Url>,
}

impl RequestHandler {
    async fn handle_request(
        self: Arc<Self>,
        req: hyper::Request<hyper::Body>,
    ) -> Result<hyper::Response<hyper::Body>, Infallible> {
        let result = self.handle_request_inner(req).await;
        match result {
            Ok(response) => Ok(response),
            Err(err) => {
                let (status_code, body) = match err {
                    ProxyError::InvalidConnectionToken => {
                        (hyper::StatusCode::FORBIDDEN, "Forbidden")
                    }
                    ProxyError::MissingHostHeader => {
                        (hyper::StatusCode::BAD_REQUEST, "Bad request")
                    }
                    ProxyError::InvalidSubdomain => {
                        (hyper::StatusCode::UNAUTHORIZED, "Invalid subdomain")
                    }
                    ProxyError::BadRequest => (hyper::StatusCode::BAD_REQUEST, "Bad request"),
                    err => {
                        tracing::error!(?err, "Unhandled error handling request.");
                        (hyper::StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
                    },
                };
                Ok(hyper::Response::builder()
                    .status(status_code)
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body(hyper::Body::from(body.to_string()))
                    .expect("Static response is always valid"))
            }
        }
    }

    async fn handle_request_inner(
        self: Arc<Self>,
        req: hyper::Request<hyper::Body>,
    ) -> Result<hyper::Response<hyper::Body>, ProxyError> {
        // Handle "/ready"
        if req.uri().path() == "/ready" {
            if self.state.connected() {
                return Ok(hyper::Response::builder()
                    .status(hyper::StatusCode::OK)
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body("Plane Proxy server (ready)".into())?);
            } else {
                return Ok(hyper::Response::builder()
                    .status(hyper::StatusCode::SERVICE_UNAVAILABLE)
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body("Plane Proxy server (not ready)".into())?);
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

            let mut uri_parts = req.uri().clone().into_parts();
            uri_parts.scheme = Some("https".parse().expect("https is a valid scheme."));
            uri_parts.authority = Some(host.parse().expect("HOST header is a valid authority."));
            uri_parts.path_and_query = uri_parts
                .path_and_query
                .or_else(|| Some(PathAndQuery::from_static("")));
            let uri = hyper::Uri::from_parts(uri_parts).expect("URI parts are valid.");
            return Ok(hyper::Response::builder()
                .status(hyper::StatusCode::MOVED_PERMANENTLY)
                .header(hyper::header::LOCATION, uri.to_string())
                .header(hyper::header::SERVER, SERVER_NAME)
                .body(hyper::Body::empty())?);
        }

        if req.uri().path() == "/" {
            if let Some(root_redirect_url) = &self.root_redirect_url {
                return Ok(hyper::Response::builder()
                    .status(hyper::StatusCode::MOVED_PERMANENTLY)
                    .header(hyper::header::LOCATION, root_redirect_url.to_string())
                    .header(hyper::header::SERVER, SERVER_NAME)
                    .body(hyper::Body::empty())?);
            }
        }

        self.handle_proxy_request(req).await
    }

    async fn handle_proxy_request(
        self: Arc<Self>,
        req: hyper::Request<hyper::Body>,
    ) -> Result<hyper::Response<hyper::Body>, ProxyError> {
        let Some(mut request_rewriter) = RequestRewriter::new(req, self.remote_meta) else {
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

        let subdomain = request_rewriter.get_subdomain(&route_info.cluster)?;
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
            let response = self.state.http_client.request(req_clone).await?;
            let response_clone = clone_response_empty_body(&response)?;

            let mut response_upgrade = hyper::upgrade::on(response).await?;
            let monitor = self.state.monitor.monitor();
            let backend_id = backend_id.clone();

            tokio::spawn(async move {
                let mut req_upgrade = match hyper::upgrade::on(req).await {
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
            self.state.http_client.request(req).await?
        };

        let headers = response.headers_mut();
        headers.insert(
            PLANE_BACKEND_ID_HEADER,
            backend_id
                .to_string()
                .parse()
                .expect("Backend ID is a valid header value."),
        );

        Ok(response)
    }
}

fn clone_response_empty_body(response: &Response<Body>) -> Result<Response<Body>, ProxyError> {
    let mut builder = Response::builder();

    builder
        .headers_mut()
        .expect("Builder::headers_mut should always work on a new builder.")
        .extend(response.headers().clone());

    builder = builder.status(response.status());

    Ok(builder.body(Body::empty()).expect("Body is always valid."))
}

pub struct ProxyService {
    handler: Arc<RequestHandler>,
}

impl Service<Request<Body>> for ProxyService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        Box::pin(self.handler.clone().handle_request(req))
    }
}

pub struct ProxyMakeService {
    pub state: Arc<ProxyState>,
    pub https_redirect: bool,
    pub root_redirect_url: Option<Url>,
}

impl ProxyMakeService {
    pub fn serve_http<F>(self, port: u16, shutdown_future: F) -> Result<JoinHandle<()>, ProxyError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        tracing::info!(%addr, "Listening for HTTP connections.");
        let server = hyper::Server::bind(&addr)
            .serve(self)
            .with_graceful_shutdown(shutdown_future);
        let handle = tokio::spawn(async {
            let _ = server.await;
        });

        Ok(handle)
    }

    pub fn serve_https<F>(
        self,
        port: u16,
        cert_watcher: CertWatcher,
        shutdown_future: F,
    ) -> Result<JoinHandle<()>, ProxyError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let server_config = ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(cert_watcher));

        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        let incoming = AddrIncoming::bind(&addr)?;
        tracing::info!(%addr, "Listening for HTTPS connections.");

        let tls_acceptor = TlsAcceptor::new(Arc::new(server_config), incoming);

        let server = hyper::Server::builder(tls_acceptor)
            .serve(self)
            .with_graceful_shutdown(shutdown_future);
        let handle = tokio::spawn(async {
            let _ = server.await;
        });

        Ok(handle)
    }
}

impl<'a> Service<&'a AddrStream> for ProxyMakeService {
    type Response = ProxyService;
    type Error = ProxyError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: &'a AddrStream) -> Self::Future {
        let remote_ip = req.remote_addr().ip();
        let handler = Arc::new(RequestHandler {
            state: self.state.clone(),
            https_redirect: self.https_redirect,
            remote_meta: ForwardableRequestInfo {
                ip: remote_ip,
                protocol: Protocol::Http,
            },
            root_redirect_url: self.root_redirect_url.clone(),
        });
        ready(Ok(ProxyService { handler })).boxed()
    }
}

impl<'a> Service<&'a TlsStream> for ProxyMakeService {
    type Response = ProxyService;
    type Error = ProxyError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: &'a TlsStream) -> Self::Future {
        let remote_ip = req.remote_ip;
        let handler = Arc::new(RequestHandler {
            state: self.state.clone(),
            https_redirect: false,
            remote_meta: ForwardableRequestInfo {
                ip: remote_ip,
                protocol: Protocol::Https,
            },
            root_redirect_url: self.root_redirect_url.clone(),
        });
        ready(Ok(ProxyService { handler })).boxed()
    }
}
