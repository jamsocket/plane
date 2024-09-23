use common::hello_world_service::HelloWorldService;
use dynamic_proxy::server::{HttpsConfig, SimpleHttpServer};
use hyper::StatusCode;
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod common;

#[tokio::test]
async fn test_hello_world_http() {
    let service = HelloWorldService;
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = SimpleHttpServer::new(service, listener, HttpsConfig::Http).unwrap();

    let url = format!("http://{}", addr);

    let client = reqwest::Client::new();
    let res = client.get(url).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await.unwrap(), "Hello, world!");
}
