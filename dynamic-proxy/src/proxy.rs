use crate::{
    body::{simple_empty_body, to_simple_body, SimpleBody},
    request::should_upgrade,
    upgrade::{split_request, split_response, UpgradeHandler},
};
use futures_util::future::BoxFuture;
use http::StatusCode;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, net::SocketAddr, time::Duration};
use tokio::net::TcpStream;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60 * 5); // 5 minutes

/// A boxed future that is responsible for proxying all communication between the client
/// and upstream server after the initial exchange of headers.
type BodyFuture = BoxFuture<'static, Result<(), ProxyError>>;

/// A client for proxying HTTP requests to an upstream server.
#[derive(Clone)]
pub struct ProxyClient {
    timeout: Duration,
}

impl Default for ProxyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyClient {
    pub fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Sends an HTTP request to the upstream server and returns a pair of the response and an
    /// optional future.
    ///
    /// If future is provided, it must be awaited to ensure that the request and response bodies
    /// are fully transferred, or if the connection is a WebSocket connection, that WebSocket
    /// messages are properly sent and received.
    pub async fn request(
        &self,
        addr: SocketAddr,
        request: Request<SimpleBody>,
    ) -> Result<(Response<SimpleBody>, Option<BodyFuture>), Infallible> {
        let url = request.uri().to_string();

        let res = self.handle_request(addr, request).await;

        let res = match res {
            Ok(res) => res,
            Err(ProxyError::Timeout) => {
                tracing::warn!(url, "Upstream request failed");
                return Ok((
                    Response::builder()
                        .status(StatusCode::GATEWAY_TIMEOUT)
                        .body(simple_empty_body())
                        .expect("Failed to build response"),
                    None,
                ));
            }
            Err(e) => {
                tracing::warn!(url, ?e, "Upstream request failed");
                return Ok((
                    Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(simple_empty_body())
                        .expect("Failed to build response"),
                    None,
                ));
            }
        };

        let (res, upgrade_handler) = res;
        let (parts, body) = res.into_parts();
        let res = Response::from_parts(parts, to_simple_body(body));

        Ok((res, upgrade_handler))
    }

    async fn handle_request(
        &self,
        addr: SocketAddr,
        request: Request<SimpleBody>,
    ) -> Result<(Response<SimpleBody>, Option<BodyFuture>), ProxyError> {
        if should_upgrade(&request) {
            let (response, upgrade_handler) = self.handle_upgrade(addr, request).await?;
            Ok((response, Some(upgrade_handler)))
        } else {
            let result = self.upstream_request(addr, request).await?;
            Ok((result, None))
        }
    }

    async fn handle_upgrade(
        &self,
        addr: SocketAddr,
        request: Request<SimpleBody>,
    ) -> Result<(Response<SimpleBody>, BodyFuture), ProxyError> {
        let (upstream_request, request_with_body) = split_request(request);
        let res = self.upstream_request(addr, upstream_request).await?;
        let (upstream_response, response_with_body) = split_response(res);

        let upgrade_handler = UpgradeHandler::new(request_with_body, response_with_body);

        Ok((upstream_response, Box::pin(upgrade_handler.run())))
    }

    async fn upstream_request(
        &self,
        addr: SocketAddr,
        request: Request<SimpleBody>,
    ) -> Result<Response<SimpleBody>, ProxyError> {
        let stream = TcpStream::connect(addr).await?;
        let io = TokioIo::new(stream);

        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(ProxyError::ConnectionHandshakeError)?;

        tokio::task::spawn(async move {
            if let Err(err) = conn.with_upgrades().await {
                tracing::warn!(?err, "Upstream connection failed.");
            }
        });

        let res = tokio::time::timeout(self.timeout, sender.send_request(request))
            .await
            .map_err(|_| ProxyError::Timeout)?
            .map_err(ProxyError::RequestSendError)?;

        let (parts, body) = res.into_parts();
        let res = Response::from_parts(parts, to_simple_body(body));

        Ok(res)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ProxyError {
    #[error("Upstream request timed out.")]
    Timeout,

    #[error("Upstream request failed: {0}")]
    RequestFailed(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("Connection handshake failed: {0}")]
    ConnectionHandshakeError(hyper::Error),

    #[error("Error sending request: {0}")]
    RequestSendError(hyper::Error),

    #[error("Failed to upgrade request: {0}")]
    RequestUpgradeError(hyper::Error),

    #[error("Failed to upgrade response: {0}")]
    ResponseUpgradeError(hyper::Error),

    #[error("IO error: {0}")]
    IoError(#[from] tokio::io::Error),
}
