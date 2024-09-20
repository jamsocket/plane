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

pub const PLANE_BACKEND_ID_HEADER: &str = "x-plane-backend-id";

pub type ProxyBody = BoxBody<Bytes, hyper::Error>;

pub fn to_proxy_body(
    body: impl Body<Data = Bytes, Error = Infallible> + Send + Sync + 'static,
) -> ProxyBody {
    #[allow(unreachable_code)]
    body.map_err(|_| unreachable!("Infallable") as hyper::Error)
        .boxed()
}

pub fn empty_proxy_body() -> ProxyBody {
    #[allow(unreachable_code)]
    Empty::new()
        .map_err(|_| unreachable!("Infallable") as hyper::Error)
        .boxed()
}

// pub type ProxyBody = Box<Incoming>;

pub const DEFAULT_CORS_HEADERS: &[(&str, &str)] = &[
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

pub fn response_builder() -> hyper::http::response::Builder {
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

pub fn box_response_body(response: Response<Incoming>) -> Response<ProxyBody> {
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, body.boxed())
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
    pub http_client: Client<HttpConnector, ProxyBody>,
    pub monitor: ConnectionMonitorHandle,
    pub connected: AtomicBool,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyState {
    pub fn new() -> Self {
        let http_client = Client::builder(TokioExecutor::new()).build_http();

        Self {
            route_map: RouteMap::new(),
            http_client,
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

pub fn clone_response_empty_body(response: &Response<Incoming>) -> Response<ProxyBody> {
    let mut builder = Response::builder();

    builder
        .headers_mut()
        .expect("Builder::headers_mut should always work on a new builder.")
        .extend(response.headers().clone());

    builder = builder.status(response.status());

    builder
        .body(empty_proxy_body())
        .expect("Response is always valid.")
}

pub struct ProxyService {
    handler: Arc<RequestHandler>,
}

impl Service<Request<ProxyBody>> for ProxyService {
    type Response = Response<ProxyBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<ProxyBody>) -> Self::Future {
        Box::pin(self.handler.clone().handle_request(req))
    }
}

pub struct ProxyMakeService {
    pub state: Arc<ProxyState>,
    pub https_redirect: bool,
    pub root_redirect_url: Option<Url>,
}

fn p(a: &impl tokio::io::AsyncRead) {}

impl ProxyMakeService {
    pub fn serve_http<F>(self, port: u16, shutdown_future: F) -> Result<JoinHandle<()>, ProxyError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();

        // let handle = tokio::spawn(async {
        //     let listener = TcpListener::bind(addr).await.unwrap(); // todo: unwrap

        //     p(&listener);
        //     todo!();

        //     let listener = TokioIo::new(listener);
        //     tracing::info!(%addr, "Listening for HTTP connections.");

        //     let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
        //     builder.serve_connection_with_upgrades(listener, self).await;
        // });

        let handle = tokio::spawn(async move {});

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
        let handle = tokio::spawn(async move {
            let server_config = ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(cert_watcher));
            let server_config = Arc::new(server_config);

            let addr: SocketAddr = ([0, 0, 0, 0], port).into();
            let tcp_listener = TcpListener::bind(addr).await.unwrap();

            // let incoming = AddrIncoming::bind(&addr).map_err(ProxyError::BindError)?;
            tracing::info!(%addr, "Listening for HTTPS connections.");

            let acceptor = TlsAcceptor::from(server_config);

            loop {
                let acceptor = acceptor.clone();
                let s = tcp_listener.accept().await;
                println!("accepted");

                let s = match s {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::error!(?err, "Error accepting connection.");
                        continue;
                    }
                };

                let (stream, _) = s;
                let stream = acceptor.accept(stream).await.unwrap();

                let stream = TokioIo::new(stream);

                hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection(stream, DummyService)
                    .await
                    .unwrap();
            }
        });

        Ok(handle)
    }
}

struct DummyService;

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for DummyService {
    type Response = Response<ProxyBody>;
    type Error = ProxyError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, _req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        Box::pin(async move {
            Ok(Response::builder()
                .body(to_proxy_body("dummy".to_string()))
                .unwrap())
        })
    }
}
