use crate::monitor::Monitor;
use anyhow::anyhow;
use core::future::Future;
use hyper::body::Bytes;
use hyper::client::HttpConnector;
use hyper::http::uri::{Authority, InvalidUriParts, Scheme};
use hyper::service::Service;
use hyper::{header, Body, Client, Request, Response, StatusCode, Uri};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::Poll;
use std::time::SystemTime;

/// Rewrite the URI from an incoming request to point to the given upstream server.
fn rewrite_uri(uri: &Uri, upstream: &Authority) -> Result<Uri, InvalidUriParts> {
    let mut parts = uri.clone().into_parts();
    parts.authority = Some(upstream.clone());
    parts.scheme = Some(Scheme::HTTP);
    Uri::from_parts(parts)
}

#[derive(Clone, Debug)]
pub struct Event {}

#[derive(Clone)]
pub struct ProxyService {
    pub client: Arc<Client<HttpConnector, Body>>,
    pub upstream: Arc<Authority>,
    pub monitor: Arc<Monitor>,
}

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

impl From<Event> for Bytes {
    fn from(_: Event) -> Self {
        Bytes::from_static(b"blah\n")
    }
}

impl ProxyService {
    pub fn new(upstream: &str) -> anyhow::Result<Self> {
        Ok(ProxyService {
            client: Arc::new(Client::new()),
            upstream: Arc::new(Authority::from_str(upstream)?),
            monitor: Arc::new(Monitor::new()),
        })
    }

    async fn handle_event_stream(self) -> anyhow::Result<Response<Body>> {
        let stream = self.monitor.status_stream();
        let body = Body::wrap_stream(stream);

        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(body)?)
    }

    async fn handle_upgrade(self, mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
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

            tokio::task::spawn(async move {
                match hyper::upgrade::on(&mut req).await {
                    Ok(mut upgraded_request) => {
                        self.monitor.open_connection();
                        let started = SystemTime::now();

                        let (from_client, from_server) = tokio::io::copy_bidirectional(
                            &mut upgraded_response,
                            &mut upgraded_request,
                        )
                        .await
                        .unwrap();

                        let duration = SystemTime::now()
                            .duration_since(started)
                            .unwrap_or_default()
                            .as_secs();

                        tracing::info!(%from_client, %from_server, ?duration, "Upgraded connection closed.");
                        self.monitor.close_connection();
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
        *req.uri_mut() = rewrite_uri(req.uri(), &self.upstream)?;

        if req.uri().path() == "/_events" {
            return self.handle_event_stream().await;
        }
        self.monitor.bump();

        if let Some(connection) = req.headers().get(hyper::http::header::CONNECTION) {
            if connection.to_str().unwrap_or_default().to_lowercase() == "upgrade" {
                tracing::info!(?connection, "Connection header");

                return self.handle_upgrade(req).await;
            }
        }
        tracing::info!(req = %req.uri(), "Handling request");
        self.client.request(req).await.map_err(|d| d.into())
    }
}

impl Service<Request<Body>> for ProxyService {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        Box::pin(Self::handle(self.clone(), req))
    }
}
