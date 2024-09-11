//! When we update to the latest hyper, these will both use the same versions of the `http`
//! crate. Until then, implementing conversions between the crates gives us some flexibility
//! around versions.

use axum::{
    body::{BoxBody, Bytes},
    extract::State,
    http::{header::HeaderMap, request, Method, Request},
    middleware::Next,
    response::Response,
};
use hyper::{StatusCode, Uri};
use reqwest::Client;
use url::Url;

pub fn convert_method(method: &Method) -> reqwest::Method {
    match method {
        &Method::GET => reqwest::Method::GET,
        &Method::POST => reqwest::Method::POST,
        &Method::PUT => reqwest::Method::PUT,
        &Method::DELETE => reqwest::Method::DELETE,
        &Method::HEAD => reqwest::Method::HEAD,
        &Method::OPTIONS => reqwest::Method::OPTIONS,
        &Method::CONNECT => reqwest::Method::CONNECT,
        &Method::PATCH => reqwest::Method::PATCH,
        &Method::TRACE => reqwest::Method::TRACE,
        _ => reqwest::Method::GET,
    }
}

pub fn convert_url(url: &Uri) -> Url {
    let url = url.to_string();
    Url::parse(&url).expect("Url is always valid.")
}

pub fn convert_header_map(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut new_headers = reqwest::header::HeaderMap::new();

    for (key, value) in headers.iter() {
        new_headers.insert(
            reqwest::header::HeaderName::from_bytes(key.as_str().as_bytes())
                .expect("HeaderName is always valid."),
            value
                .to_str()
                .expect("Header value is always valid.")
                .parse()
                .unwrap(),
        );
    }

    new_headers
}

pub fn clone_request_with_empty_body(parts: &request::Parts) -> reqwest::Request {
    // Copy method and URL.
    let method = convert_method(&parts.method);
    let url = convert_url(&parts.uri);
    let mut request = reqwest::Request::new(method, url);

    // Copy headers.
    let headers = request.headers_mut();

    headers.extend(convert_header_map(&parts.headers));

    headers.insert(
        "x-original-path",
        reqwest::header::HeaderValue::from_str(parts.uri.path()).expect("Path is always valid."),
    );

    request
}

pub async fn forward_layer<B>(
    State(forward_url): State<Url>,
    req: Request<B>,
    next: Next<B>,
) -> Response<BoxBody> {
    let (parts, body) = req.into_parts();
    let mut forward_req = clone_request_with_empty_body(&parts);
    let req = Request::from_parts(parts, body);

    let uri = forward_url
        .to_string()
        .parse::<Url>()
        .expect("Url should always parse as hyper Uri.");
    *forward_req.url_mut() = uri;

    // Create a client
    let client = Client::new();

    // Forward the request
    let forwarded_resp = client.execute(forward_req).await;

    let forwarded_resp = match forwarded_resp {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!(?err, "Error forwarding auth.");
            return response_helper(StatusCode::BAD_GATEWAY, b"Error forwarding auth.");
        }
    };

    if forwarded_resp.status().is_success() {
        next.run(req).await
    } else {
        response_helper(StatusCode::UNAUTHORIZED, b"Unauthorized")
    }
}

fn response_helper(status: StatusCode, body: &'static [u8]) -> Response<BoxBody> {
    // This is a bit ugly. There seems to be no way to construct an http_body with an axum::Error error type (?),
    // but we can use map_err from http_body::Body to convert the hyper::error::Error to an axum::Error.
    // Then, we need to box it up for Axum.
    let body = http_body::Full::new(Bytes::from_static(body));
    let body = http_body::Body::map_err(body, axum::Error::new);
    let body: BoxBody = BoxBody::new(body);

    Response::builder()
        .status(status.as_u16())
        .body(body)
        .expect("Response is always valid.")
}
