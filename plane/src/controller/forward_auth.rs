use axum::{
    body::{Body, BoxBody, Bytes},
    extract::State,
    http::{request, HeaderValue, Request},
    middleware::Next,
    response::Response,
};
use hyper::{Client, StatusCode, Uri};
use url::Url;

pub fn clone_request_with_empty_body(parts: &request::Parts) -> request::Request<Body> {
    // Copy method and URL.
    let mut builder = request::Builder::new()
        .method(parts.method.clone())
        .uri(parts.uri.clone());

    // Copy headers.
    let headers = builder
        .headers_mut()
        .expect("Can always call headers_mut() on a new builder.");

    headers.extend(parts.headers.clone());

    headers.insert(
        "x-original-path",
        HeaderValue::from_str(parts.uri.path()).expect("Path is always valid."),
    );

    // Construct with an empty body.
    builder
        .body(Body::empty())
        .expect("Request is always valid.")
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
        .parse::<Uri>()
        .expect("Url should always parse as hyper Uri.");
    *forward_req.uri_mut() = uri;

    // Create a client
    let client = Client::new();

    // Forward the request
    let forwarded_resp = client.request(forward_req).await;

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
