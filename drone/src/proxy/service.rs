use super::connection_tracker::ConnectionTracker;
use super::tls::TlsStream;
use super::PLANE_AUTH_COOKIE;
use crate::database::{DroneDatabase, ProxyRoute};
use anyhow::{anyhow, Context, Result};
use http::uri::{Authority, PathAndQuery, Scheme};
use http::Uri;
use hyper::client::HttpConnector;
use hyper::server::conn::AddrStream;
use hyper::Client;
use hyper::{service::Service, Body, Request, Response, StatusCode};
use serde::Deserialize;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
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
    passthrough: Option<SocketAddr>,
    allow_path_routing: bool,
}

impl MakeProxyService {
    pub fn new(
        db: DroneDatabase,
        cluster: String,
        connection_tracker: ConnectionTracker,
        passthrough: Option<SocketAddr>,
        allow_path_routing: bool,
    ) -> Self {
        MakeProxyService {
            db,
            client: Client::new(),
            cluster,
            connection_tracker,
            passthrough,
            allow_path_routing,
        }
    }
}

impl<'a> Service<&'a AddrStream> for MakeProxyService {
    type Response = ProxyService;
    type Error = Infallible;
    type Future = Ready<Result<ProxyService, Infallible>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: &'a AddrStream) -> Self::Future {
        let remote_ip = req.remote_addr().ip();
        ready(Ok(ProxyService {
            db: self.db.clone(),
            client: self.client.clone(),
            cluster: self.cluster.clone(),
            connection_tracker: self.connection_tracker.clone(),
            remote_ip,
            passthrough: self.passthrough,
            allow_path_routing: self.allow_path_routing,
        }))
    }
}

impl<'a> Service<&'a TlsStream> for MakeProxyService {
    type Response = ProxyService;
    type Error = Infallible;
    type Future = Ready<Result<ProxyService, Infallible>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: &'a TlsStream) -> Self::Future {
        let remote_ip = req.remote_ip;
        ready(Ok(ProxyService {
            db: self.db.clone(),
            client: self.client.clone(),
            cluster: self.cluster.clone(),
            connection_tracker: self.connection_tracker.clone(),
            remote_ip,
            passthrough: self.passthrough,
            allow_path_routing: self.allow_path_routing,
        }))
    }
}

#[derive(Clone)]
pub struct ProxyService {
    db: DroneDatabase,
    client: Client<HttpConnector, Body>,
    cluster: String,
    connection_tracker: ConnectionTracker,
    remote_ip: IpAddr,
    passthrough: Option<SocketAddr>,
    allow_path_routing: bool,
}

fn check_auth<T>(req: &Request<T>, expected_token: &str) -> Result<Option<Response<Body>>> {
    let req_bearer_token = req.headers().get(http::header::AUTHORIZATION);

    if let Some(req_bearer_token) = req_bearer_token {
        let token_bytes = req_bearer_token.as_bytes();
        if (&token_bytes[0..7] == b"Bearer " || &token_bytes[0..7] == b"bearer ")
            && &token_bytes[7..] == expected_token.as_bytes()
        {
            return Ok(None);
        } else {
            return Ok(Some(
                Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .body(Body::empty())
                    .unwrap(),
            ));
        }
    }

    let cookies = req.headers().get_all(http::header::COOKIE);
    let expected_prefix = format!("{}=", PLANE_AUTH_COOKIE);

    for cookie in cookies {
        let cookie = cookie.to_str().unwrap();

        for cookie in cookie.split(';') {
            let cookie = cookie.trim();

            if let Some(cookie) = cookie.strip_prefix(&expected_prefix) {
                if cookie == expected_token {
                    return Ok(None);
                }
            }
        }
    }

    let result = Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(Body::from("Expected bearer token."))?;

    Ok(Some(result))
}

#[allow(unused)]
impl ProxyService {
    fn rewrite_uri(authority: &str, uri: &Uri) -> anyhow::Result<Uri> {
        let mut parts = uri.clone().into_parts();
        parts.authority = Some(Authority::from_str(authority)?);
        parts.scheme = Some(Scheme::HTTP);
        parts.path_and_query = parts
            .path_and_query
            .or(Some(PathAndQuery::from_static("/")));
        let uri = Uri::from_parts(parts).context("Error rewriting proxy URL.")?;

        Ok(uri)
    }

    async fn handle_upgrade(
        self,
        mut req: Request<Body>,
        backend: &str,
    ) -> anyhow::Result<Response<Body>> {
        let response = self
            .client
            .request(clone_request(&req).context("Error cloning request.")?)
            .await
            .context("Error making upstream proxy request.")?;

        if response.status() == StatusCode::SWITCHING_PROTOCOLS {
            let response_clone = clone_response(&response).context("Error cloning response.")?;

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
                        let duration = SystemTime::now()
                            .duration_since(started)
                            .unwrap_or_default()
                            .as_secs();

                        match result {
                            Ok((from_client, from_server)) => {
                                tracing::info!(%from_client, %from_server, ?duration, "Upgraded connection closed.");
                            }
                            Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
                                tracing::info!(
                                    ?duration,
                                    "Upgraded connection closed with UnexpectedEof."
                                );
                            }
                            Err(error) if error.kind() == ErrorKind::TimedOut => {
                                tracing::info!(?duration, "Upgraded connection timed out.");
                            }
                            Err(error) => {
                                tracing::error!(
                                    ?duration,
                                    ?error,
                                    "Error with upgraded connection."
                                );
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

    fn backend_from_request(&self, req: &mut Request<Body>) -> Result<String> {
        let mut uri = req.uri_mut();
        let path = uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default();

        if self.allow_path_routing {
            if let Some(rest) = path.strip_prefix("/_plane_backend=") {
                let (backend, new_path) = rest.split_once('/').unwrap_or((rest, ""));
                let new_path = format!("/{}", new_path);

                // Replace path with a version that strips the /_plane_backend=... prefix.
                let mut parts = uri.clone().into_parts();

                let p = http::uri::PathAndQuery::from_str(&new_path).unwrap();

                parts.path_and_query = Some(
                    http::uri::PathAndQuery::from_str(&new_path)
                        .context("Error parsing path and query.")?,
                );
                *uri = Uri::from_parts(parts).context("Error rewriting proxy URL.")?;

                return Ok(backend.to_string());
            } else {
                return Err(anyhow!("allow_path_routing is set but path ({}) does not have /_plane_backend= as prefix", path));
            }
        }

        if let Some(host) = req.headers().get(http::header::HOST) {
            let host = std::str::from_utf8(host.as_bytes())?;
            let host_trimmed = host.rsplit_once(':').map(|(host, _)| host).unwrap_or(host);
            let host_trimmed = host_trimmed.to_lowercase();
            let Some(subdomain) = host_trimmed.strip_suffix(&format!(".{}", self.cluster)) else {
                tracing::warn!(%host, %path, "Unable to parse backend from Host header.");
                return Err(anyhow!("Host does not end with cluster domain."));
            };
            return Ok(subdomain.to_owned());
        }

        Err(anyhow!("No backend found in host or URL path."))
    }

    fn handle_plane_auth(&self, req: Request<Body>) -> anyhow::Result<Response<Body>> {
        let params: PlaneAuthParams =
            serde_html_form::from_str(req.uri().query().unwrap_or_default())?;

        if let Some(redirect) = params.redirect.as_deref() {
            if !redirect.starts_with('/') {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body("Redirect must be relative and start with a slash.".into())
                    .unwrap());
            }
        }

        Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", params.redirect.as_deref().unwrap_or("/"))
            .header(
                "Set-Cookie",
                format!("{}={}", PLANE_AUTH_COOKIE, params.token),
            )
            .body(Body::empty())?)
    }

    async fn handle(self, mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
        let subdomain = match self.backend_from_request(&mut req) {
            Ok(subdomain) => subdomain,
            Err(error) => {
                tracing::warn!(?error, "Error getting backend from request.");
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Body::empty())?);
            }
        };

        tracing::info!(ip=%self.remote_ip, url=%req.uri(), "Proxy Request");
        let route = self.db.get_proxy_route(&subdomain).await?.or_else(|| {
            self.passthrough.map(|d| ProxyRoute {
                address: d.to_string(),
                bearer_token: None,
            })
        });

        if req.uri().path() == "/_plane_auth" {
            return self.handle_plane_auth(req);
        }

        if let Some(proxy_route) = route {
            self.connection_tracker.track_request(&subdomain);
            *req.uri_mut() = Self::rewrite_uri(&proxy_route.address, req.uri())?;

            if let Some(token) = proxy_route.bearer_token {
                if let Some(response) = check_auth(&req, &token)? {
                    return Ok(response);
                }
            }

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

            let result = self
                .client
                .request(req)
                .await
                .context("Error handling client request.")?;
            return Ok(result);
        }

        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
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

#[derive(Deserialize)]
struct PlaneAuthParams {
    token: String,
    redirect: Option<String>,
}
