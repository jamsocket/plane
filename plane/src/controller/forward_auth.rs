use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::Uri,
    http::{request, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use url::Url;

pub fn clone_request_with_empty_body(parts: &request::Parts) -> hyper::http::Request<Body> {
    // Copy method and URL.
    let mut builder = hyper::http::request::Builder::new()
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

pub async fn forward_layer(State(forward_url): State<Url>, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let mut forward_req = clone_request_with_empty_body(&parts);
    let req = Request::from_parts(parts, body);

    let uri = forward_url
        .to_string()
        .parse::<Uri>()
        .expect("Url should always parse as hyper Uri.");
    *forward_req.uri_mut() = uri;

    // Create a client
    let client = Client::builder(TokioExecutor::new()).build(HttpConnector::new());

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

fn response_helper(status: StatusCode, body: &'static [u8]) -> Response {
    let body = Body::from(Bytes::from_static(body));

    Response::builder()
        .status(status.as_u16())
        .body(body)
        .expect("Response is always valid.")
}
