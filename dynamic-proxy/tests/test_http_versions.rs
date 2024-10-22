use common::hello_world_service::HelloWorldService;
use plane_dynamic_proxy::server::{HttpsConfig, SimpleHttpServer};
use hyper::StatusCode;
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod common;

#[tokio::test]
async fn test_http1() {
    let service = HelloWorldService;
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = SimpleHttpServer::new(service, listener, HttpsConfig::Http).unwrap();

    let url = format!("http://{}", addr);

    let client = reqwest::Client::builder().http1_only().build().unwrap();
    let res = client.get(url).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.version(), reqwest::Version::HTTP_11);
    assert_eq!(
        res.text().await.unwrap(),
        "Hello, world! X-Forwarded-For: 127.0.0.1, X-Forwarded-Proto: http"
    );
}

#[tokio::test]
async fn test_http2() {
    let service = HelloWorldService;
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = SimpleHttpServer::new(service, listener, HttpsConfig::Http).unwrap();

    let url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let res = client.get(url).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.version(), reqwest::Version::HTTP_2);
    assert_eq!(
        res.text().await.unwrap(),
        "Hello, world! X-Forwarded-For: 127.0.0.1, X-Forwarded-Proto: http"
    );
}
