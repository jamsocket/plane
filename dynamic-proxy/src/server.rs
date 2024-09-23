use crate::{body::SimpleBody, graceful_shutdown::GracefulShutdown};
use anyhow::Result;
use hyper::{body::Incoming, service::Service, Request, Response};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder as ServerBuilder,
};
use rustls::{server::ResolvesServerCert, ServerConfig};
use std::{sync::Arc, time::Duration};
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

        let stream = match stream {
            Ok((stream, _)) => stream,
            Err(e) => {
                tracing::warn!(?e, "Failed to accept connection.");
                continue;
            }
        };
        let service = service.clone();

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

        let stream = match stream {
            Ok((stream, _)) => stream,
            Err(e) => {
                tracing::warn!(?e, "Failed to accept connection.");
                continue;
            }
        };
        let service = service.clone();
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
