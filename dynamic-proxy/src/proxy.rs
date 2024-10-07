use crate::{
    body::{simple_empty_body, to_simple_body, SimpleBody},
    request::should_upgrade,
    upgrade::{split_request, split_response, UpgradeHandler},
};
use http::StatusCode;
use hyper::{Request, Response};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use std::{convert::Infallible, time::Duration};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A client for proxying HTTP requests to an upstream server.
#[derive(Clone)]
pub struct ProxyClient {
    client: Client<HttpConnector, SimpleBody>,
    timeout: Duration,
}

impl Default for ProxyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyClient {
    pub fn new() -> Self {
        let client = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        Self {
            client,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Sends an HTTP request to the upstream server and returns the response.
    /// If the request establishes a websocket connection, an upgrade handler is returned.
    /// In this case, you must call and await `.run()` on the upgrade handler (i.e. in a tokio task)
    /// to ensure that messages are properly sent and received.
    pub async fn request(
        &self,
        request: Request<SimpleBody>,
    ) -> Result<(Response<SimpleBody>, Option<UpgradeHandler>), Infallible> {
        let url = request.uri().to_string();

        let res = self.handle_request(request).await;

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
        request: Request<SimpleBody>,
    ) -> Result<(Response<SimpleBody>, Option<UpgradeHandler>), ProxyError> {
        if should_upgrade(&request) {
            let (response, upgrade_handler) = self.handle_upgrade(request).await?;
            Ok((response, Some(upgrade_handler)))
        } else {
            let result = self.upstream_request(request).await?;
            Ok((result, None))
        }
    }

    async fn handle_upgrade(
        &self,
        request: Request<SimpleBody>,
    ) -> Result<(Response<SimpleBody>, UpgradeHandler), ProxyError> {
        let (upstream_request, request_with_body) = split_request(request);
        let res = self.upstream_request(upstream_request).await?;
        let (upstream_response, response_with_body) = split_response(res);

        let upgrade_handler = UpgradeHandler::new(request_with_body, response_with_body);

        Ok((upstream_response, upgrade_handler))
    }

    async fn upstream_request(
        &self,
        request: Request<SimpleBody>,
    ) -> Result<Response<SimpleBody>, ProxyError> {
        let res = match tokio::time::timeout(self.timeout, self.client.request(request)).await {
            Ok(Ok(res)) => res,
            Err(_) => {
                return Err(ProxyError::Timeout);
            }
            Ok(Err(e)) => {
                return Err(ProxyError::RequestFailed(e.into()));
            }
        };

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

    #[error("Failed to upgrade response: {0}")]
    UpgradeError(#[from] hyper::Error),

    #[error("IO error: {0}")]
    IoError(#[from] tokio::io::Error),
}
