use common::{cert::StaticCertificateResolver, hello_world_service::HelloWorldService};
use dynamic_proxy::server::{HttpsConfig, SimpleHttpServer};
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod common;

#[tokio::test]
async fn test_https() {
    let resolver = StaticCertificateResolver::new();
    let cert = resolver.certificate();
    let hostname = resolver.hostname();

    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = SimpleHttpServer::new(
        HelloWorldService,
        listener,
        HttpsConfig::from_resolver(resolver),
    )
    .unwrap();

    let client = reqwest::Client::builder()
        .https_only(true)
        .add_root_certificate(cert)
        .resolve(&hostname, addr /* port is ignored */)
        .build()
        .unwrap();

    let url = format!("https://{}:{}", hostname, addr.port());

    let res = client.get(&url).send().await.unwrap();
    assert!(res.status().is_success());
    assert_eq!(
        res.text().await.unwrap(),
        "Hello, world! X-Forwarded-For: 127.0.0.1, X-Forwarded-Proto: https"
    );
}
