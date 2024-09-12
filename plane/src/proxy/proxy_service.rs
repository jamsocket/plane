use super::connection_monitor::ConnectionMonitorHandle;
use super::rewriter::RequestRewriterError;
use super::route_map::RouteMap;
use super::tls::TlsStream;
use super::{ForwardableRequestInfo, Protocol};
use crate::names::BackendName;
use crate::proxy::cert_manager::CertWatcher;
use crate::proxy::rewriter::RequestRewriter;
use crate::proxy::tls::TlsAcceptor;
use crate::SERVER_NAME;
use axum::http::uri::PathAndQuery;
use futures_util::{Future, FutureExt};
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::upgrade::Upgraded;
use hyper::{
    body::{Body, Bytes},
    service::Service,
    Request, Response,
};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{atomic::AtomicBool, Arc};
use std::task::Context;
use std::{
    future::ready,
    io::ErrorKind,
    task::{self, Poll},
};
use tokio::io::{copy_bidirectional, AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::ServerConfig;
use url::Url;

const PLANE_BACKEND_ID_HEADER: &str = "x-plane-backend-id";

pub type ProxyBody = Box<dyn Body<Data = Bytes, Error = hyper::Error> + Unpin + Send>;

pub fn to_proxy_body(body: impl Body<Data = Bytes, Error = Infallible>) -> ProxyBody {
    body.boxed()
}

pub fn empty_proxy_body() -> ProxyBody {
    Empty::new().boxed()
}

// pub type ProxyBody = Box<Incoming>;

const DEFAULT_CORS_HEADERS: &[(&str, &str)] = &[
    ("Access-Control-Allow-Origin", "*"),
    (
        "Access-Control-Allow-Methods",
        "GET, POST, PUT, DELETE, OPTIONS",
    ),
    (
        "Access-Control-Allow-Headers",
        "Content-Type, Authorization",
    ),
    ("Access-Control-Allow-Credentials", "true"),
];

fn response_builder() -> hyper::http::response::Builder {
    let mut request = hyper::Response::builder();
    request = request.header("Access-Control-Allow-Origin", "*");
    request = request.header(
        "Access-Control-Allow-Methods",
        "GET, POST, PUT, DELETE, OPTIONS",
    );
    request = request.header(
        "Access-Control-Allow-Headers",
        "Content-Type, Authorization",
    );
    request = request.header("Access-Control-Allow-Credentials", "true");
    request
}

fn box_response_body(response: Response<Incoming>) -> Response<ProxyBody> {
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, Box::new(body))
}

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

    #[error("Error binding server: {0}")]
    BindError(hyper::Error),

    #[error("Error upgrading request: {0}")]
    UpgradeError(hyper::Error),

    #[error("Error making request: {0} (backend: {1})")]
    RequestError(hyper_util::client::legacy::Error, BackendName),

    #[error("Error making upgradable (legacy type error) request: {0}")]
    UpgradableRequestLegacyError(hyper_util::client::legacy::Error),

    #[error("Error making upgradable request: {0}")]
    UpgradableRequestError(hyper::Error),
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
    http_client: Client<HttpConnector, ProxyBody>,
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
            http_client: Client::new(),
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

struct WrappedUpgrade {
    upgrade: Upgraded,
}

/// Adapted from:
/// https://github.com/hyperium/hyper/blob/e3e707ea2abaeb98e42c31259d867547c7890a35/benches/support/tokiort.rs#L102-L124
impl AsyncRead for WrappedUpgrade {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        tbuf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        //let init = tbuf.initialized().len();
        let filled = tbuf.filled().len();
        let sub_filled = unsafe {
            let mut buf = hyper::rt::ReadBuf::uninit(tbuf.unfilled_mut());

            match hyper::rt::Read::poll_read(self.project().inner, cx, buf.unfilled()) {
                Poll::Ready(Ok(())) => buf.filled().len(),
                other => return other,
            }
        };

        let n_filled = filled + sub_filled;
        // At least sub_filled bytes had to have been initialized.
        let n_init = sub_filled;
        unsafe {
            tbuf.assume_init(n_init);
            tbuf.set_filled(n_filled);
        }

        Poll::Ready(Ok(()))
    }
}

/// Adapted from:
/// https://github.com/hyperium/hyper/blob/e3e707ea2abaeb98e42c31259d867547c7890a35/benches/support/tokiort.rs#L194-L228
impl AsyncWrite for WrappedUpgrade {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        hyper::rt::Write::poll_write(self.project().inner, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        hyper::rt::Write::poll_flush(self.project().inner, cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        hyper::rt::Write::poll_shutdown(self.project().inner, cx)
    }

    fn is_write_vectored(&self) -> bool {
        hyper::rt::Write::is_write_vectored(&self.inner)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        hyper::rt::Write::poll_write_vectored(self.project().inner, cx, bufs)
    }
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

                let mut response_upgrade = WrappedUpgrade {
                    upgrade: response_upgrade,
                };

                let mut req_upgrade = WrappedUpgrade {
                    upgrade: req_upgrade,
                };

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

fn clone_response_empty_body(response: &Response<Incoming>) -> Response<ProxyBody> {
    let mut builder = Response::builder();

    builder
        .headers_mut()
        .expect("Builder::headers_mut should always work on a new builder.")
        .extend(response.headers().clone());

    builder = builder.status(response.status());

    builder
        .body(Body::empty())
        .expect("Response is always valid.")
}

pub struct ProxyService {
    handler: Arc<RequestHandler>,
}

impl Service<Request<ProxyBody>> for ProxyService {
    type Response = Response<ProxyBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&mut self, req: Request<ProxyBody>) -> Self::Future {
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

        let handle = tokio::spawn(async {
            let listener = TcpListener::bind(addr).await?;
            tracing::info!(%addr, "Listening for HTTP connections.");

            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());

            builder.serve_connection_with_upgrades(listener, self).await;
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
        let incoming = AddrIncoming::bind(&addr).map_err(ProxyError::BindError)?;
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
