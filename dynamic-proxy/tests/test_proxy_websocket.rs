use common::websocket_echo_server::WebSocketEchoServer;
use dynamic_proxy::{
    body::SimpleBody,
    proxy::ProxyClient,
    request::MutableRequest,
    server::{HttpsConfig, SimpleHttpServer},
};
use futures_util::{SinkExt, StreamExt};
use http::{Request, Response};
use hyper::{body::Incoming, service::Service};
use std::{future::Future, net::SocketAddr, pin::Pin};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

mod common;

#[derive(Clone)]
pub struct SimpleProxyService {
    upstream: SocketAddr,
    client: ProxyClient,
}

impl SimpleProxyService {
    pub fn new(upstream: SocketAddr) -> Self {
        let client = ProxyClient::new();
        Self { upstream, client }
    }
}

impl Service<Request<Incoming>> for SimpleProxyService {
    type Response = Response<SimpleBody>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = Pin<
        Box<
            dyn Future<
                    Output = Result<Response<SimpleBody>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        let request = MutableRequest::from_request(request);
        let request = request.into_request_with_simple_body();
        let client = self.client.clone();

        let upstream = self.upstream;
        Box::pin(async move {
            let (res, body_future) = client.request(upstream, request).await.unwrap();

            let body_future = body_future.unwrap();
            tokio::spawn(async move {
                body_future.await.unwrap();
            });

            Ok(res)
        })
    }
}

#[tokio::test]
async fn test_websocket_echo() {
    // Start the WebSocket echo server
    let server = WebSocketEchoServer::new().await;
    let server_addr = server.addr();

    // Start the proxy
    let proxy_service = SimpleProxyService::new(server_addr);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind listener");
    let proxy_addr = listener.local_addr().expect("Failed to get proxy address");
    let _server = SimpleHttpServer::new(proxy_service, listener, HttpsConfig::Http).unwrap();

    // Connect to the WebSocket server
    let url = format!("ws://{}/ws", proxy_addr);
    let (mut ws_stream, _) = connect_async(&url).await.expect("Failed to connect");

    // Send a message
    let message = "Hello, WebSocket!";
    ws_stream
        .send(Message::Text(message.to_string()))
        .await
        .expect("Failed to send message");

    // Receive the echoed message
    if let Some(Ok(msg)) = ws_stream.next().await {
        match msg {
            Message::Text(received_text) => {
                assert_eq!(
                    received_text, message,
                    "Received message doesn't match sent message"
                );
            }
            _ => panic!("Unexpected message type received"),
        }
    } else {
        panic!("Failed to receive message");
    }

    // Close the connection
    ws_stream
        .close(None)
        .await
        .expect("Failed to close WebSocket");
}
