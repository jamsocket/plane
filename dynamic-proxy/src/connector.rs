use core::task;
use http::Uri;
use hyper_util::{client::legacy::connect::HttpConnector, rt::TokioIo};
use std::{future::Future, pin::Pin, task::Poll, time::Duration};
use tokio::net::TcpStream;

#[derive(Clone)]
pub struct TimeoutHttpConnector {
    pub timeout: Duration,
    pub connector: HttpConnector,
}

impl Default for TimeoutHttpConnector {
    fn default() -> Self {
        TimeoutHttpConnector {
            timeout: Duration::from_secs(10),
            connector: HttpConnector::new(),
        }
    }
}

impl tower_service::Service<Uri> for TimeoutHttpConnector {
    type Response = TokioIo<TcpStream>;
    type Error = TimeoutHttpConnectorError;
    type Future = Pin<Box<dyn Future<Output = Result<TokioIo<TcpStream>, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.connector
            .poll_ready(cx)
            .map_err(|e| TimeoutHttpConnectorError::Boxed(Box::new(e)))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let fut = self.connector.call(dst);
        let timeout = self.timeout;
        Box::pin(async move {
            let result = tokio::time::timeout(timeout, fut).await;
            match result {
                Ok(Ok(io)) => Ok(io),
                Ok(Err(e)) => Err(TimeoutHttpConnectorError::Boxed(Box::new(e))),
                Err(_) => Err(TimeoutHttpConnectorError::Timeout),
            }
        })
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TimeoutHttpConnectorError {
    #[error("Timeout")]
    Timeout,

    #[error("Non-timeout error: {0}")]
    Boxed(#[from] Box<dyn std::error::Error + Send + Sync>),
}
