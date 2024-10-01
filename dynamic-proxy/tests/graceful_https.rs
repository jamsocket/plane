use bytes::Bytes;
use common::cert::StaticCertificateResolver;
use dynamic_proxy::body::to_simple_body;
use dynamic_proxy::server::HttpsConfig;
use dynamic_proxy::server::SimpleHttpServer;
use hyper::StatusCode;
use std::convert::Infallible;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::time::Duration;

mod common;

// Ref: https://github.com/hyperium/hyper-util/blob/master/examples/server_graceful.rs

#[tokio::test]
async fn test_graceful_shutdown_https() {
    // Set up HTTPS configuration
    let resolver = StaticCertificateResolver::new();
    let cert = resolver.certificate();
    let hostname = resolver.hostname();

    // Start the server
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = SimpleHttpServer::new(
        hyper::service::service_fn(|_| async move {
            tokio::time::sleep(Duration::from_secs(1)).await; // emulate slow request
            let body = http_body_util::Full::<Bytes>::from("Hello, world!".to_owned());
            let body = to_simple_body(body);
            Ok::<_, Infallible>(hyper::Response::new(body))
        }),
        listener,
        HttpsConfig::from_resolver(resolver),
    )
    .unwrap();

    let url = format!("https://{}:{}", hostname, addr.port());

    // Create a client with HTTPS configuration
    let client = reqwest::Client::builder()
        .https_only(true)
        .add_root_certificate(cert)
        .resolve(&hostname, addr /* port is ignored */)
        .build()
        .unwrap();

    let response_handle = {
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move { client.get(&url).send().await.unwrap() })
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Call server.graceful_shutdown()
    let shutdown_task = tokio::spawn(async move { server.graceful_shutdown().await });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let response = response_handle.await.unwrap();

    // Wait for the shutdown task to complete.
    shutdown_task.await.unwrap();

    // Ensure that the result is as expected
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), "Hello, world!");

    // Attempt to make another request, which should fail due to the server shutting down
    let result = client.get(&url).send().await;
    assert!(result.is_err());
}
