use super::connection_tracker::ConnectionTracker;
use crate::database::DroneDatabase;
use anyhow::{anyhow, Result};
use http::uri::{Authority, Scheme};
use http::Uri;
use hyper::client::HttpConnector;
use hyper::Client;
use hyper::{service::Service, Body, Request, Response, StatusCode};
use std::str::FromStr;
use std::time::SystemTime;
use std::{
    convert::Infallible,
    future::{ready, Future, Ready},
    pin::Pin,
    task::Poll,
};

const UPGRADE: &str = "upgrade";

/// Clone a request (method and headers, not body).
fn clone_request(request: &Request<Body>) -> Result<Request<Body>, hyper::http::Error> {
    let mut builder = Request::builder();
    builder = builder.uri(request.uri());
    for (key, value) in request.headers() {
        builder = builder.header(key, value);
    }
    builder = builder.method(request.method());
    builder.body(Body::empty())
}

/// Clone a response (status and headers, not body).
fn clone_response(response: &Response<Body>) -> Result<Response<Body>, hyper::http::Error> {
    let mut builder = Response::builder();
    for (key, value) in response.headers() {
        builder = builder.header(key, value);
    }
    builder = builder.status(response.status());
    builder.body(Body::empty())
}

pub struct MakeProxyService {
    db: DroneDatabase,
    client: Client<HttpConnector, Body>,
    cluster: String,
    connection_tracker: ConnectionTracker,
}

impl MakeProxyService {
    pub fn new(db: DroneDatabase, cluster: String, connection_tracker: ConnectionTracker) -> Self {
        MakeProxyService {
            db,
            client: Client::new(),
            cluster,
            connection_tracker,
        }
    }
}

impl<T> Service<T> for MakeProxyService {
    type Response = ProxyService;
    type Error = Infallible;
    type Future = Ready<Result<ProxyService, Infallible>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: T) -> Self::Future {
        ready(Ok(ProxyService {
            db: self.db.clone(),
            client: self.client.clone(),
            cluster: self.cluster.clone(),
            connection_tracker: self.connection_tracker.clone(),
        }))
    }
}

#[derive(Clone)]
pub struct ProxyService {
    db: DroneDatabase,
    client: Client<HttpConnector, Body>,
    cluster: String,
    connection_tracker: ConnectionTracker,
}

#[allow(unused)]
impl ProxyService {
    fn rewrite_uri(authority: &str, uri: &Uri) -> anyhow::Result<Uri> {
        let mut parts = uri.clone().into_parts();
        parts.authority = Some(Authority::from_str(authority)?);
        parts.scheme = Some(Scheme::HTTP);
        let uri = Uri::from_parts(parts)?;

        Ok(uri)
    }

    async fn handle_upgrade(
        self,
        mut req: Request<Body>,
        backend: &str,
    ) -> anyhow::Result<Response<Body>> {
        let response = self.client.request(clone_request(&req)?).await?;

        if response.status() == StatusCode::SWITCHING_PROTOCOLS {
            let response_clone = clone_response(&response)?;

            let mut upgraded_response = match hyper::upgrade::on(response).await {
                Ok(upgraded) => upgraded,
                Err(e) => {
                    tracing::error!(?e, "Error upgrading response.");
                    return Err(anyhow!("Upgrade error."));
                }
            };

            let connection_tracker = self.connection_tracker.clone();
            let backend = backend.to_string();
            tokio::task::spawn(async move {
                match hyper::upgrade::on(&mut req).await {
                    Ok(mut upgraded_request) => {
                        let started = SystemTime::now();

                        connection_tracker.increment_connections(&backend);
                        let result = tokio::io::copy_bidirectional(
                            &mut upgraded_response,
                            &mut upgraded_request,
                        )
                        .await;
                        connection_tracker.decrement_connections(&backend);

                        match result {
                            Ok((from_client, from_server)) => {
                                let duration = SystemTime::now()
                                    .duration_since(started)
                                    .unwrap_or_default()
                                    .as_secs();

                                tracing::info!(%from_client, %from_server, ?duration, "Upgraded connection closed.");
                            }
                            Err(error) => {
                                tracing::error!(?error, "IO error upgrading connection.");
                            }
                        }
                    }
                    Err(e) => tracing::error!(?e, "Error upgrading request."),
                }
            });

            Ok(response_clone)
        } else {
            Ok(response)
        }
    }

    async fn handle(self, mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
        if let Some(host) = req.headers().get(http::header::HOST) {
            let host = std::str::from_utf8(host.as_bytes())?;

            // TODO: we shouldn't need to allocate a string just to strip a prefix.
            if let Some(subdomain) = host.strip_suffix(&format!(".{}", self.cluster)) {
                let subdomain = subdomain.to_string();
                if let Some(addr) = self.db.get_proxy_route(&subdomain).await? {
                    self.connection_tracker.track_request(&subdomain);
                    *req.uri_mut() = Self::rewrite_uri(&addr, req.uri())?;

                    if let Some(connection) = req.headers().get(hyper::http::header::CONNECTION) {
                        if connection
                            .to_str()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(UPGRADE)
                        {
                            return self.handle_upgrade(req, &subdomain).await;
                        }
                    }

                    let result = self.client.request(req).await;
                    return Ok(result?);
                }
            }

            tracing::warn!(?host, "Unrecognized host.");
        }

        tracing::warn!("No host header present on request.");

        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())?)
    }

    async fn warn_handle(self, mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
        let result = self.handle(req).await;

        if let Err(error) = &result {
            tracing::warn!(?error, "Error handling request.")
        }

        result
    }
}

type ProxyServiceFuture =
    Pin<Box<dyn Future<Output = Result<Response<Body>, anyhow::Error>> + Send + 'static>>;

impl Service<Request<Body>> for ProxyService {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future = ProxyServiceFuture;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        Box::pin(self.clone().warn_handle(req))
    }
}
