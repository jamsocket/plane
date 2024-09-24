use crate::{
    body::SimpleBody, graceful_shutdown::GracefulShutdown, https_redirect::HttpsRedirectService,
};
use anyhow::Result;
use http::HeaderValue;
use hyper::{body::Incoming, service::Service, Request, Response};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder as ServerBuilder,
};
use rustls::{server::ResolvesServerCert, ServerConfig};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, select, task::JoinSet};
use tokio_rustls::TlsAcceptor;

pub struct SimpleHttpServer {
    handle: tokio::task::JoinHandle<JoinSet<()>>,
    graceful_shutdown: Option<GracefulShutdown>,
}

async fn listen_loop<S>(
    listener: TcpListener,
    service: S,
    graceful_shutdown: GracefulShutdown,
) -> JoinSet<()>
where
    S: Service<Request<Incoming>, Response = Response<SimpleBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let mut recv = graceful_shutdown.subscribe();
    let mut join_set = JoinSet::new();

    loop {
        let stream = select! {
            stream = listener.accept() => stream,
            _ = recv.changed() => break,
        };

        let (stream, remote_addr) = match stream {
            Ok((stream, remote_addr)) => (stream, remote_addr),
            Err(e) => {
                tracing::warn!(?e, "Failed to accept connection.");
                continue;
            }
        };
        let remote_ip = remote_addr.ip();
        let service = WrappedService::new(service.clone(), remote_ip, "http");

        let server = ServerBuilder::new(TokioExecutor::new());
        let io = TokioIo::new(stream);
        let conn = server.serve_connection_with_upgrades(io, service);

        let conn = graceful_shutdown.watch(conn.into_owned());
        join_set.spawn(async {
            if let Err(e) = conn.await {
                tracing::warn!(?e, "Failed to serve connection.");
            }
        });
    }

    // Even though join_set is never used, we return it to keep it from being dropped
    // until the graceful shutdown (or timeout) is complete. Otherwise, the tasks we started
    // would be stopped as soon as the graceful shutdown is initiated.
    join_set
}

async fn listen_loop_tls<S>(
    listener: TcpListener,
    service: S,
    resolver: Arc<dyn ResolvesServerCert>,
    graceful_shutdown: GracefulShutdown,
) -> JoinSet<()>
where
    S: Service<Request<Incoming>, Response = Response<SimpleBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));
    let mut recv = graceful_shutdown.subscribe();
    let mut join_set = JoinSet::new();

    loop {
        let stream = select! {
            stream = listener.accept() => stream,
            _ = recv.changed() => break,
        };

        let (stream, remote_addr) = match stream {
            Ok((stream, remote_addr)) => (stream, remote_addr),
            Err(e) => {
                tracing::warn!(?e, "Failed to accept connection.");
                continue;
            }
        };
        let remote_ip = remote_addr.ip();
        let service = WrappedService::new(service.clone(), remote_ip, "https");
        let tls_acceptor = tls_acceptor.clone();

        let graceful_shutdown = graceful_shutdown.clone();
        join_set.spawn(async move {
            let server = ServerBuilder::new(TokioExecutor::new());

            let stream = match tls_acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::warn!(?e, "Failed to accept TLS connection.");
                    return;
                }
            };
            let io = TokioIo::new(stream);

            let conn = server.serve_connection_with_upgrades(io, service);
            let conn = graceful_shutdown.watch(conn.into_owned());

            if let Err(e) = conn.await {
                tracing::warn!(?e, "Failed to serve connection.");
            }
        });
    }

    // Even though join_set is never used, we return it to keep it from being dropped
    // until the graceful shutdown (or timeout) is complete. Otherwise, the tasks we started
    // would be stopped as soon as the graceful shutdown is initiated.
    join_set
}

pub enum HttpsConfig {
    Http,
    Https {
        resolver: Arc<dyn ResolvesServerCert>,
    },
}

impl HttpsConfig {
    pub fn from_resolver<R: ResolvesServerCert + 'static>(resolver: R) -> Self {
        Self::Https {
            resolver: Arc::new(resolver),
        }
    }

    pub fn http() -> Self {
        Self::Http
    }
}

impl SimpleHttpServer {
    pub fn new<S>(service: S, listener: TcpListener, https_config: HttpsConfig) -> Result<Self>
    where
        S: Service<Request<Incoming>, Response = Response<SimpleBody>> + Clone + Send + 'static,
        S::Future: Send + 'static,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let graceful_shutdown = GracefulShutdown::new();

        let handle = match https_config {
            HttpsConfig::Http => {
                tokio::spawn(listen_loop(listener, service, graceful_shutdown.clone()))
            }
            HttpsConfig::Https { resolver } => {
                if rustls::crypto::ring::default_provider()
                    .install_default()
                    .is_err()
                {
                    tracing::info!("Using already-installed crypto provider.")
                }

                tokio::spawn(listen_loop_tls(
                    listener,
                    service,
                    resolver,
                    graceful_shutdown.clone(),
                ))
            }
        };

        Ok(Self {
            handle,
            graceful_shutdown: Some(graceful_shutdown),
        })
    }

    pub async fn graceful_shutdown(mut self) {
        println!("Shutting down");
        let graceful_shutdown = self
            .graceful_shutdown
            .take()
            .expect("self.graceful_shutdown is always set");
        graceful_shutdown.shutdown().await;
    }

    pub async fn graceful_shutdown_with_timeout(mut self, timeout: Duration) {
        let graceful_shutdown = self
            .graceful_shutdown
            .take()
            .expect("self.graceful_shutdown is always set");
        tokio::time::timeout(timeout, graceful_shutdown.shutdown())
            .await
            .unwrap();
    }
}

impl Drop for SimpleHttpServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub struct ServerWithHttpRedirect {
    http_server: SimpleHttpServer,
    https_server: Option<SimpleHttpServer>,
}

pub struct ServerWithHttpRedirectHttpsConfig {
    pub https_port: u16,
    pub resolver: Arc<dyn ResolvesServerCert>,
}

pub struct ServerWithHttpRedirectConfig {
    pub http_port: u16,
    pub https_config: Option<ServerWithHttpRedirectHttpsConfig>,
}

impl ServerWithHttpRedirect {
    pub async fn new<S>(service: S, server_config: ServerWithHttpRedirectConfig) -> Result<Self>
    where
        S: Service<Request<Incoming>, Response = Response<SimpleBody>>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        if let Some(https_config) = server_config.https_config {
            // Serve HTTPS
            let https_listener =
                TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], https_config.https_port)))
                    .await?;
            let https_server = SimpleHttpServer::new(
                service,
                https_listener,
                HttpsConfig::Https {
                    resolver: https_config.resolver,
                },
            )?;

            // Redirect HTTP to HTTPS
            let http_listener =
                TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], server_config.http_port)))
                    .await?;
            let http_server =
                SimpleHttpServer::new(HttpsRedirectService, http_listener, HttpsConfig::Http)?;

            Ok(Self {
                http_server,
                https_server: Some(https_server),
            })
        } else {
            let listener =
                TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], server_config.http_port)))
                    .await?;
            let http_server = SimpleHttpServer::new(service, listener, HttpsConfig::Http)?;

            Ok(Self {
                http_server,
                https_server: None,
            })
        }
    }

    pub async fn graceful_shutdown_with_timeout(self, timeout: Duration) {
        if let Some(https_server) = self.https_server {
            tokio::join!(
                self.http_server.graceful_shutdown_with_timeout(timeout),
                https_server.graceful_shutdown_with_timeout(timeout)
            );
        } else {
            self.http_server
                .graceful_shutdown_with_timeout(timeout)
                .await;
        }
    }
}

/// A service that wraps another service and sets
/// X-Forwarded-For and X-Forwarded-Proto headers.
struct WrappedService<S> {
    inner: S,
    forwarded_for: IpAddr,
    forwarded_proto: &'static str,
}

impl<S> WrappedService<S> {
    pub fn new(inner: S, forwarded_for: IpAddr, forwarded_proto: &'static str) -> Self {
        Self {
            inner,
            forwarded_for,
            forwarded_proto,
        }
    }
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for WrappedService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, request: Request<ReqBody>) -> Self::Future {
        let mut request = request;
        request.headers_mut().insert(
            "X-Forwarded-For",
            HeaderValue::from_str(&format!("{}", self.forwarded_for)).unwrap(),
        );
        request.headers_mut().insert(
            "X-Forwarded-Proto",
            HeaderValue::from_str(self.forwarded_proto).unwrap(),
        );
        self.inner.call(request)
    }
}
