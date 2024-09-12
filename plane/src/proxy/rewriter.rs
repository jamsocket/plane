use super::{
    proxy_service::{empty_proxy_body, ProxyBody},
    subdomain::subdomain_from_host,
    ForwardableRequestInfo,
};
use crate::{
    protocol::RouteInfo,
    types::{BearerToken, ClusterName},
};
use hyper::{
    header::{HeaderValue, HOST},
    http::{request, uri},
    HeaderMap, Request, Uri,
};
use std::{borrow::BorrowMut, net::SocketAddr, str::FromStr};
use tungstenite::http::uri::PathAndQuery;

const VERIFIED_HEADER_PREFIX: &str = "x-verified-";
const USERNAME_HEADER: &str = "x-verified-username";
const AUTH_SECRET_HEADER: &str = "x-verified-secret";
const AUTH_USER_DATA_HEADER: &str = "x-verified-user-data";
const PATH_PREFIX_HEADER: &str = "x-verified-path";
const BACKEND_ID_HEADER: &str = "x-verified-backend";
const X_FORWARDED_FOR_HEADER: &str = "x-forwarded-for";
const X_FORWARDED_PROTO_HEADER: &str = "x-forwarded-proto";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RequestRewriterError {
    #[error("Invalid `host` header")]
    InvalidHostHeader,
}

impl From<hyper::header::ToStrError> for RequestRewriterError {
    fn from(_: hyper::header::ToStrError) -> Self {
        RequestRewriterError::InvalidHostHeader
    }
}

pub struct RequestRewriter {
    parts: request::Parts,
    uri_parts: uri::Parts,
    body: ProxyBody,
    bearer_token: BearerToken,
    prefix_uri: Uri,
    remote_meta: ForwardableRequestInfo,
}

impl RequestRewriter {
    pub fn new(request: Request<ProxyBody>, remote_meta: ForwardableRequestInfo) -> Option<Self> {
        let (parts, body) = request.into_parts();

        let mut uri_parts = parts.uri.clone().into_parts();
        uri_parts.scheme = Some("http".parse().expect("Scheme is valid."));

        let bearer_token = match extract_bearer_token(&mut uri_parts) {
            Some(bearer_token) => bearer_token,
            None => {
                tracing::warn!(uri=?parts.uri, "Bearer token not found in URI.");
                return None;
            }
        };

        let mut prefix_uri_parts = parts.uri.clone().into_parts();
        prefix_uri_parts.path_and_query =
            Some(PathAndQuery::from_str(&format!("/{}/", bearer_token)).expect("Path is valid."));
        let prefix_uri = Uri::from_parts(prefix_uri_parts).expect("URI parts are valid.");

        Some(Self {
            parts,
            uri_parts,
            body,
            bearer_token,
            prefix_uri,
            remote_meta,
        })
    }

    pub fn set_authority(&mut self, addr: SocketAddr) {
        self.uri_parts.authority = Some(
            addr.to_string()
                .parse()
                .expect("SocketAddr is a valid authority."),
        );
    }

    pub fn bearer_token(&self) -> &BearerToken {
        &self.bearer_token
    }

    /// Returns the subdomain of the request's host header, after stripping the cluster name.
    /// Returns Ok(Some(subdomain)) if a subdomain is found.
    /// Returns Ok(None) if no subdomain is found, but the host header matches the cluster name.
    /// Returns Err(RequestRewriterError::InvalidHostHeader) if the host header does not
    /// match the cluster name, or no host header is found.
    pub fn get_subdomain(
        &self,
        cluster: &ClusterName,
    ) -> Result<Option<&str>, RequestRewriterError> {
        let Some(hostname) = self.parts.headers.get(HOST) else {
            return Err(RequestRewriterError::InvalidHostHeader);
        };

        let hostname = match hostname.to_str() {
            Ok(hostname) => hostname,
            Err(err) => {
                tracing::warn!(?hostname, ?err, "Host header is not valid UTF-8.");
                return Err(RequestRewriterError::InvalidHostHeader);
            }
        };

        subdomain_from_host(hostname, cluster)
    }

    fn into_parts(self) -> (request::Parts, ProxyBody, Uri, ForwardableRequestInfo) {
        let Self {
            mut parts,
            uri_parts,
            body,
            prefix_uri,
            remote_meta,
            ..
        } = self;

        let uri = Uri::from_parts(uri_parts).expect("URI parts are valid.");
        parts.uri = uri;

        (parts, body, prefix_uri, remote_meta)
    }

    pub fn into_request(self, route_info: &RouteInfo) -> Request<ProxyBody> {
        let (mut parts, body, prefix_uri, remote_meta) = self.into_parts();

        let headers = parts.headers.borrow_mut();
        set_headers_from_route_info(headers, route_info, &prefix_uri, remote_meta);

        Request::from_parts(parts, body)
    }

    pub fn into_request_pair(
        self,
        route_info: &RouteInfo,
    ) -> (Request<ProxyBody>, Request<ProxyBody>) {
        let (parts, body, prefix_uri, remote_meta) = self.into_parts();
        let req2 = clone_request_with_empty_body(&parts, route_info, &prefix_uri, remote_meta);
        let req1 = Request::from_parts(parts, body);

        (req1, req2)
    }

    pub fn should_upgrade(&self) -> bool {
        let Some(conn_header) = self.parts.headers.get("connection") else {
            return false;
        };

        let Ok(conn_header) = conn_header.to_str() else {
            return false;
        };

        conn_header
            .to_lowercase()
            .split(',')
            .any(|s| s.trim() == "upgrade")
    }
}

fn clone_request_with_empty_body(
    parts: &request::Parts,
    route_info: &RouteInfo,
    prefix_uri: &Uri,
    remote_meta: ForwardableRequestInfo,
) -> request::Request<ProxyBody> {
    let mut builder = request::Builder::new()
        .method(parts.method.clone())
        .uri(parts.uri.clone());

    let headers = builder
        .headers_mut()
        .expect("Can always call headers_mut() on a new builder.");

    headers.extend(parts.headers.clone());
    set_headers_from_route_info(headers, route_info, prefix_uri, remote_meta);

    builder
        .body(empty_proxy_body())
        .expect("Request is always valid.")
}

fn extract_bearer_token(parts: &mut uri::Parts) -> Option<BearerToken> {
    let Some(path_and_query) = parts.path_and_query.clone() else {
        panic!("No path and query");
    };

    let full_path = path_and_query.path().strip_prefix('/')?;

    // Split the incoming path into the token and the path to proxy to. If there is no slash, the token is
    // the full incoming path, and the path to proxy to is just `/`.
    let (token, path) = match full_path.split_once('/') {
        Some((token, path)) => (token, path),
        None => (full_path, "/"),
    };

    let token = BearerToken::from(token.to_string());

    if token.is_static() {
        // We don't rewrite the URL if using a static token.
        return Some(token);
    }

    let query = path_and_query
        .query()
        .map(|query| format!("?{}", query))
        .unwrap_or_default();

    parts.path_and_query = Some(
        PathAndQuery::from_str(format!("/{}{}", path, query).as_str())
            .expect("Path and query is valid."),
    );

    Some(token)
}

fn set_headers_from_route_info(
    headers: &mut HeaderMap,
    route_info: &RouteInfo,
    prefix_uri: &Uri,
    remote_meta: ForwardableRequestInfo,
) {
    let mut headers_to_remove = Vec::new();
    for header_name in headers.keys() {
        if header_name.as_str().starts_with(VERIFIED_HEADER_PREFIX) {
            headers_to_remove.push(header_name.clone());
        }
    }

    for header_name in headers_to_remove {
        headers.remove(header_name);
    }

    if let Some(user) = &route_info.user {
        headers.insert(
            USERNAME_HEADER,
            HeaderValue::from_str(user.as_str()).expect("User is valid."),
        );
    }

    let forwards = if let Some(forwards) = headers.get(X_FORWARDED_FOR_HEADER) {
        let forwards = forwards.to_str().unwrap_or("").to_string();
        format!("{}, {}", forwards, remote_meta.ip)
    } else {
        remote_meta.ip.to_string()
    };

    headers.insert(
        X_FORWARDED_FOR_HEADER,
        HeaderValue::from_str(forwards.as_str()).expect("Forwards are valid."),
    );

    if headers.get(X_FORWARDED_PROTO_HEADER).is_none() {
        headers.insert(
            "x-forwarded-proto",
            HeaderValue::from_static(remote_meta.protocol.as_str()),
        );
    }

    headers.insert(
        AUTH_SECRET_HEADER,
        HeaderValue::from_str(&route_info.secret_token.to_string()).expect("Secret is valid."),
    );

    headers.insert(
        AUTH_USER_DATA_HEADER,
        HeaderValue::from_str(
            &serde_json::to_string(&route_info.user_data)
                .expect("JSON value should always serialize."),
        )
        .expect("User data is valid"),
    );

    headers.insert(
        PATH_PREFIX_HEADER,
        HeaderValue::from_str(&prefix_uri.to_string()).expect("Path is valid."),
    );

    headers.insert(
        BACKEND_ID_HEADER,
        route_info
            .backend_id
            .to_string()
            .parse()
            .expect("Backend ID is a valid header value."),
    );
}
