use bytes::Bytes;
use common::simple_upgrade_service::SimpleUpgradeService;
use dynamic_proxy::server::{HttpsConfig, SimpleHttpServer};
use http_body_util::Empty;
use hyper::{
    header::{HeaderValue, UPGRADE, CONNECTION},
    Request, StatusCode,
};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

mod common;

#[tokio::test]
async fn test_upgrade() {
    let service = SimpleUpgradeService;
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = SimpleHttpServer::new(service, listener, HttpsConfig::Http).unwrap();

    let url = format!("http://{}", addr);

    let req = Request::builder()
        .uri(url)
        .header(UPGRADE, "websocket")
        .body(Empty::<Bytes>::new())
        .unwrap();

    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();

    let handle = tokio::task::spawn(async move {
        // conn.with_upgrades() will block until sender.send_request() is called.
        // It's not clear to me why, but the example to run it in its own task
        // comes from this example:
        // https://github.com/hyperium/hyper/blob/master/examples/upgrades.rs
        if let Err(err) = conn.with_upgrades().await {
            Err(anyhow::anyhow!("Connection failed: {:?}", err))
        } else {
            Ok(())
        }
    });

    let res = sender.send_request(req).await.unwrap();
    handle.await.unwrap().unwrap();

    assert_eq!(res.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        res.headers().get(UPGRADE).unwrap(),
        &HeaderValue::from_static("websocket")
    );
    assert_eq!(
        res.headers().get(CONNECTION).unwrap(),
        &HeaderValue::from_static("upgrade")
    );

    let upgraded = hyper::upgrade::on(res).await.unwrap();
    let mut upgraded = TokioIo::new(upgraded);
    upgraded.write_all(b"Hello from the client!").await.unwrap();

    let mut buf = vec![0; 1024];
    let n = upgraded.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"Hello from the client!");

    upgraded.flush().await.unwrap();
}
