use anyhow::Result;
use axum::{body::Body, http::Request};
use std::sync::{Arc, Mutex};
use tokio::{sync::{mpsc, oneshot}, task::JoinHandle};
use url::Url;

#[allow(unused)]
pub struct AuthRequest {
    request: Request<Body>,
    reply_channel: Option<oneshot::Sender<bool>>,
}

#[allow(unused)]
impl AuthRequest {
    fn reply(&mut self, ok: bool) {
        let reply_channel = self
            .reply_channel
            .take()
            .expect("Can only reply to an AuthRequest once");
        reply_channel.send(ok).unwrap();
    }

    pub fn accept(&mut self) {
        self.reply(true);
    }

    pub fn reject(&mut self) {
        self.reply(false);
    }

    pub fn request(&self) -> &Request<Body> {
        &self.request
    }
}

#[allow(unused)]
pub struct MockAuthServer {
    handle: JoinHandle<()>,
    waiting: Arc<Mutex<mpsc::Receiver<AuthRequest>>>,
    url: Url,
}

#[allow(unused)]
impl MockAuthServer {
    pub async fn new() -> Self {
        let (tx, rx) = mpsc::channel(1);
        let waiting: Arc<Mutex<mpsc::Receiver<AuthRequest>>> = Arc::new(Mutex::new(rx));
        let waiting_clone = Arc::clone(&waiting);

        let app = axum::Router::new().route(
            "/auth",
            axum::routing::any(move |request: Request<Body>| async move {
                let (reply_tx, reply_rx) = oneshot::channel();
                {
                    let auth_request = AuthRequest {
                        request,
                        reply_channel: Some(reply_tx),
                    };

                    tx.send(auth_request).await.unwrap();
                }

                let is_authorized = reply_rx.await.unwrap();
                if is_authorized {
                    axum::http::StatusCode::OK
                } else {
                    axum::http::StatusCode::UNAUTHORIZED
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://127.0.0.1:{}/auth",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let server = axum::serve(listener, app);

        let handle = tokio::spawn(async move {
            server.await.unwrap();
        });

        Self {
            handle,
            waiting,
            url,
        }
    }

    pub fn url(&self) -> Url {
        self.url.clone()
    }

    /// Expect the server to have received an auth request, and returns a handle that can be used to
    /// accept or reject the request.
    pub async fn expect(&mut self) -> Result<AuthRequest> {
        let mut waiting = self.waiting.lock().unwrap();
        let auth_request = waiting.recv().await.unwrap();
        Ok(auth_request)
    }
}

impl Drop for MockAuthServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
