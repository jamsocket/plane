use crate::{
    controller::error::ApiError,
    names::{BackendName, DroneName},
    protocol::{MessageFromDns, MessageFromDrone, MessageFromProxy},
    typed_socket::client::TypedSocketConnector,
    types::{ClusterId, ConnectRequest, ConnectResponse, TimestampedBackendStatus},
};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

mod sse;

#[derive(thiserror::Error, Debug)]
pub enum PlaneClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Unexpected status code: {0}")]
    UnexpectedStatus(StatusCode),

    #[error("API error: {0} ({1})")]
    PlaneError(ApiError, StatusCode),
}

pub struct PlaneClient {
    client: reqwest::Client,
    base_url: Url,
}

async fn get_response<T: DeserializeOwned>(response: Response) -> Result<T, PlaneClientError> {
    if response.status().is_success() {
        Ok(response.json::<T>().await?)
    } else {
        let url = response.url().to_string();
        tracing::error!(?url, "Got error response from API server.");
        let status = response.status();
        if let Ok(api_error) = response.json::<ApiError>().await {
            Err(PlaneClientError::PlaneError(api_error, status))
        } else {
            Err(PlaneClientError::UnexpectedStatus(status))
        }
    }
}

fn http_to_ws_url(url: &mut Url) {
    if url.scheme() == "http" {
        url.set_scheme("ws")
            .expect("should always be able to set URL scheme to static value ws");
    } else if url.scheme() == "https" {
        url.set_scheme("wss")
            .expect("should always be able to set URL scheme to static value wss");
    }
}

impl PlaneClient {
    pub fn new(base_url: Url) -> Self {
        let client = reqwest::Client::new();
        Self { client, base_url }
    }

    pub async fn status(&self) -> Result<(), PlaneClientError> {
        let url = self.base_url.join("/ctrl/status")?;

        let response = self.client.get(url).send().await?;
        get_response::<Value>(response).await?;
        Ok(())
    }

    pub fn drone_connection(&self, cluster: &ClusterId) -> TypedSocketConnector<MessageFromDrone> {
        let mut url = self
            .base_url
            .join(&format!("/ctrl/c/{}/drone-socket", cluster))
            .expect("url is always valid");
        http_to_ws_url(&mut url);
        TypedSocketConnector::new(url)
    }

    pub fn proxy_connection(&self, cluster: &ClusterId) -> TypedSocketConnector<MessageFromProxy> {
        let mut url = self
            .base_url
            .join(&format!("/ctrl/c/{}/proxy-socket", cluster))
            .expect("url is always valid");
        http_to_ws_url(&mut url);
        TypedSocketConnector::new(url)
    }

    pub fn dns_connection(&self) -> TypedSocketConnector<MessageFromDns> {
        let mut url = self
            .base_url
            .join("/ctrl/dns-socket")
            .expect("url is always valid");
        http_to_ws_url(&mut url);
        TypedSocketConnector::new(url)
    }

    pub async fn connect(
        &self,
        cluster: &ClusterId,
        connect_request: &ConnectRequest,
    ) -> Result<ConnectResponse, PlaneClientError> {
        let url = self
            .base_url
            .join(&format!("/ctrl/c/{}/connect", cluster))?;

        let respose = self.client.post(url).json(connect_request).send().await?;
        let connect_response: ConnectResponse = get_response(respose).await?;
        Ok(connect_response)
    }

    pub async fn drain(
        &self,
        cluster: &ClusterId,
        drone: &DroneName,
    ) -> Result<(), PlaneClientError> {
        let url = self
            .base_url
            .join(&format!("/ctrl/c/{}/d/{}/drain", cluster, drone))?;

        let response = self.client.post(url).send().await?;
        get_response::<Value>(response).await?;
        Ok(())
    }

    pub async fn soft_terminate(
        &self,
        cluster: &ClusterId,
        backend_id: &BackendName,
    ) -> Result<(), PlaneClientError> {
        let url = self.base_url.join(&format!(
            "/ctrl/c/{}/b/{}/soft-terminate",
            cluster, backend_id
        ))?;

        let response = self.client.post(url).send().await?;
        get_response::<Value>(response).await?;
        Ok(())
    }

    pub async fn hard_terminate(
        &self,
        cluster: &ClusterId,
        backend_id: &BackendName,
    ) -> Result<(), PlaneClientError> {
        let url = self.base_url.join(&format!(
            "/ctrl/c/{}/b/{}/hard-terminate",
            cluster, backend_id
        ))?;

        let response = self.client.post(url).send().await?;
        get_response::<Value>(response).await?;
        Ok(())
    }

    pub async fn backend_status(
        &self,
        cluster: &ClusterId,
        backend_id: &BackendName,
    ) -> Result<TimestampedBackendStatus, PlaneClientError> {
        let url = self
            .base_url
            .join(&format!("/pub/c/{}/b/{}/status", cluster, backend_id))?;

        let response = self.client.get(url).send().await?;
        let status: TimestampedBackendStatus = get_response(response).await?;
        Ok(status)
    }

    pub async fn backend_status_stream(
        &self,
        cluster: &ClusterId,
        backend_id: &BackendName,
    ) -> Result<sse::SseStream<TimestampedBackendStatus>, PlaneClientError> {
        let url = self.base_url.join(&format!(
            "/pub/c/{}/b/{}/status-stream",
            cluster, backend_id
        ))?;

        let stream = sse::sse_request(url, self.client.clone()).await?;
        Ok(stream)
    }
}
