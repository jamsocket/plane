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
use std::time::Duration;

pub struct ProxyClient {
    client: Client<HttpConnector, SimpleBody>,
    timeout: Duration,
}

impl Clone for ProxyClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            timeout: self.timeout,
        }
    }
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
            timeout: Duration::from_secs(10),
        }
    }

    pub async fn request(
        &self,
        request: Request<SimpleBody>,
    ) -> Result<
        (Response<SimpleBody>, Option<UpgradeHandler>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let url = request.uri().to_string();

        let res = self.handle_request(request).await;

        let res = match res {
            Ok(res) => res,
            Err(ProxyError::Timeout) => {
                tracing::error!(url, "Upstream request failed");
                return Ok((
                    Response::builder()
                        .status(StatusCode::GATEWAY_TIMEOUT)
                        .body(simple_empty_body())
                        .unwrap(),
                    None,
                ));
            }
            Err(e) => {
                tracing::error!(url, ?e, "Upstream request failed");
                return Ok((
                    Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(simple_empty_body())
                        .unwrap(),
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
            println!("Upgrading request");
            let (response, upgrade_handler) = self.handle_upgrade(request).await?;
            println!("Upgraded request");
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
        let url = request.uri().to_string();

        let res = match tokio::time::timeout(self.timeout, self.client.request(request)).await {
            Ok(Ok(res)) => res,
            Err(_) => {
                tracing::warn!(url, "Upstream request timed out.");
                return Err(ProxyError::Timeout);
            }
            Ok(Err(e)) => {
                tracing::error!(url, "Upstream request failed: {}", e);
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
