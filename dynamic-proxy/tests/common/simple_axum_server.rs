use axum::{body::Body, extract::Request, routing::any, Json, Router};
use http::Method;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr};
use tokio::net::TcpListener;

/// A simple server that returns the request info as json.
pub struct SimpleAxumServer {
    handle: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
}

#[allow(unused)]
impl SimpleAxumServer {
    pub async fn new() -> Self {
        let app = Router::new()
            .route("/*path", any(return_request_info))
            .route("/", any(return_request_info));

        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let tcp_listener = TcpListener::bind(addr).await.unwrap();
        let addr = tcp_listener.local_addr().unwrap();

        let handle = tokio::spawn(async {
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

impl Drop for SimpleAxumServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// Handler function for the root route
async fn return_request_info(method: Method, request: Request<Body>) -> Json<RequestInfo> {
    let method = method.to_string();

    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or("").to_string();

    let headers: HashMap<String, String> = request
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
        .collect();

    let body = request.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();

    Json(RequestInfo {
        path,
        query,
        method,
        headers,
        body,
    })
}

#[derive(Serialize, Deserialize)]
pub struct RequestInfo {
    pub path: String,
    pub query: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}
