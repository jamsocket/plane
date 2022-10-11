use crate::util::random_loopback_ip;
use anyhow::{anyhow, Result};
use futures::Future;
use futures::{future, StreamExt, TryStreamExt};
use hyper::service::{make_service_fn, service_fn};
use hyper::{self, Body, Request, Response};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub struct Server {
    server_handle: JoinHandle<()>,
    pub address: SocketAddr,
}

impl Server {
    pub async fn new<F, T>(handle_inner: F) -> Result<Self>
    where
        F: Fn(Request<Body>) -> T + Send + Sync + 'static,
        T: Future<Output = String> + Send + Sync + 'static,
    {
        let ip = random_loopback_ip();
        let address = SocketAddr::new(ip.into(), 8080);
        let handle_inner = Arc::new(handle_inner);

        let make_svc = make_service_fn(move |_conn| {
            let handle_inner = handle_inner.clone();
            async {
                let wrapped_handler = move |r| {
                    let handle_inner = handle_inner.clone();
                    async move { Ok::<_, Infallible>(Response::new(Body::from(handle_inner(r).await))) }
                };
                Ok::<_, Infallible>(service_fn(wrapped_handler))
            }
        });

        let server = hyper::Server::bind(&address).serve(make_svc);

        let server_handle = tokio::spawn(async {
            server.await.unwrap();
        });

        let server = Server {
            server_handle,
            address,
        };
        server.wait_ready().await?;
        Ok(server)
    }

    pub async fn serve_web_sockets() -> Result<Self> {
        let ip = random_loopback_ip();
        let address = SocketAddr::new(ip.into(), 8080);

        let tcp_listener = TcpListener::bind(address).await.expect("failed to bind");

        let server_handle = tokio::spawn(async move {
            while let Ok((stream, _)) = tcp_listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("Error during the websocket handshake occurred");
                let (write, read) = ws_stream.split();
                read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
                    .forward(write)
                    .await
                    .expect("Failed to forward messages");
            }
        });

        let server = Server {
            server_handle,
            address,
        };

        Ok(server)
    }

    async fn wait_ready(&self) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10_000))
            .unwrap();

        loop {
            let url = format!("http://{}/", self.address);
            let result = tokio::time::timeout_at(deadline, reqwest::get(&url)).await;
            match result {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(_)) => (), // Not ready yet.
                Err(_) => return Err(anyhow!("Timed out before ready.")),
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.server_handle.abort();
    }
}
