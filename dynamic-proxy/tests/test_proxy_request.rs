use crate::common::simple_axum_server::SimpleAxumServer;
use anyhow::Result;
use bytes::Bytes;
use common::simple_axum_server::RequestInfo;
use dynamic_proxy::{
    body::{simple_empty_body, to_simple_body, BoxedError},
    proxy::ProxyClient,
    request::MutableRequest,
};
use http::{Method, Request, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod common;

async fn make_request(req: Request<BoxBody<Bytes, BoxedError>>) -> Result<RequestInfo> {
    let server = SimpleAxumServer::new().await;
    let proxy_client = ProxyClient::new();

    let mut req = MutableRequest::from_request(req);
    let host = req.parts.uri.host().unwrap().to_string();
    req.add_header("host", &host);

    let (res, upgrade_handler) = proxy_client
        .request(server.addr(), req.into_request())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(upgrade_handler.is_none());

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let result: RequestInfo = serde_json::from_slice(&body).unwrap();

    Ok(result)
}

#[tokio::test]
async fn test_proxy_simple_request() {
    let req = Request::builder()
        .method(Method::GET)
        .uri("http://foo.bar".to_string())
        .body(simple_empty_body())
        .unwrap();

    let result = make_request(req).await.unwrap();

    assert_eq!(result.path, "/");
    assert_eq!(result.method, "GET");
    assert_eq!(result.headers.len(), 1);
    assert!(result.headers.contains_key("host"));
}

#[tokio::test]
async fn test_proxy_simple_post_request() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("http://foo.bar".to_string())
        .body(simple_empty_body())
        .unwrap();

    let result = make_request(req).await.unwrap();

    assert_eq!(result.path, "/");
    assert_eq!(result.method, "POST");
    assert_eq!(result.headers.len(), 1);
    assert!(result.headers.contains_key("host"));
}

#[tokio::test]
async fn test_proxy_request_with_path_and_query_params() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("http://foo.bar/foo/bar?baz=1&qux=2".to_string())
        .body(simple_empty_body())
        .unwrap();

    let result = make_request(req).await.unwrap();

    assert_eq!(result.path, "/foo/bar");
    assert_eq!(result.query, "baz=1&qux=2");
    assert_eq!(result.method, "POST");
    assert_eq!(result.headers.len(), 1);
    assert!(result.headers.contains_key("host"));
}

#[tokio::test]
async fn test_proxy_request_with_headers() {
    let req = Request::builder()
        .method(Method::GET)
        .uri("http://foo.bar/foo".to_string())
        .header("X-Test", "test")
        .body(simple_empty_body())
        .unwrap();

    let result = make_request(req).await.unwrap();

    assert_eq!(result.path, "/foo");
    assert_eq!(result.method, "GET");
    assert_eq!(result.headers.len(), 2);
    assert!(result.headers.contains_key("host"));
    assert_eq!(result.headers.get("x-test").unwrap(), "test");
}

#[tokio::test]
async fn test_proxy_body() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("http://foo.bar/foo".to_string())
        .body(to_simple_body(Full::new("test".into())))
        .unwrap();

    let result = make_request(req).await.unwrap();

    assert_eq!(result.path, "/foo");
    assert_eq!(result.method, "POST");
    assert_eq!(result.headers.len(), 2);
    assert!(result.headers.contains_key("host"));
    assert!(result.headers.contains_key("content-length"));
    assert_eq!(result.body, "test");
}

#[tokio::test]
async fn test_proxy_no_upstream() {
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let tcp_listener = TcpListener::bind(addr).await.unwrap();
    let addr = tcp_listener.local_addr().unwrap();
    drop(tcp_listener);

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("http://{}", addr))
        .body(simple_empty_body())
        .unwrap();

    let client = ProxyClient::new();
    let (result, upgrade_handler) = client.request(addr, req).await.unwrap();

    assert_eq!(result.status(), StatusCode::BAD_GATEWAY);
    assert!(upgrade_handler.is_none());
}
