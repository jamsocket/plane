use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// A websocket echo server that echos messages back to the client.
pub struct WebSocketEchoServer {
    handle: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
}

#[allow(unused)]
impl WebSocketEchoServer {
    pub async fn new() -> Self {
        let app = Router::new().route("/ws", get(ws_handler));

        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let tcp_listener = TcpListener::bind(addr).await.unwrap();
        let addr = tcp_listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(tcp_listener, app.into_make_service())
                .await
                .unwrap();
        });

        Self { handle, addr }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for WebSocketEchoServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(msg) => {
                if let Message::Text(text) = msg {
                    if socket.send(Message::Text(text)).await.is_err() {
                        panic!("WebSocket connection closed.");
                    }
                }
            }
            Err(e) => {
                panic!("Error receiving message: {}", e);
            }
        }
    }
}
