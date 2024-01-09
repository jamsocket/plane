use self::controller_address::AuthorizedAddress;
use crate::{
    controller::{error::ApiError, StatusResponse},
    names::{BackendName, DroneName},
    protocol::{MessageFromDns, MessageFromDrone, MessageFromProxy},
    typed_socket::client::TypedSocketConnector,
    types::{ClusterName, ConnectRequest, ConnectResponse, TimestampedBackendStatus},
};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

pub mod controller_address;
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

#[derive(Clone)]
pub struct PlaneClient {
    client: reqwest::Client,
    controller_address: AuthorizedAddress,
}

impl PlaneClient {
    pub fn new(base_url: Url) -> Self {
        let client = reqwest::Client::new();
        let controller_address = AuthorizedAddress::from(base_url);

        Self {
            client,
            controller_address,
        }
    }

    pub async fn status(&self) -> Result<StatusResponse, PlaneClientError> {
        let addr = self.controller_address.join("/ctrl/status");
        authed_get(&self.client, &addr).await
    }

    pub fn drone_connection(
        &self,
        cluster: &ClusterName,
    ) -> TypedSocketConnector<MessageFromDrone> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/c/{}/drone-socket", cluster))
            .to_websocket_address();
        TypedSocketConnector::new(addr)
    }

    pub fn proxy_connection(
        &self,
        cluster: &ClusterName,
    ) -> TypedSocketConnector<MessageFromProxy> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/c/{}/proxy-socket", cluster))
            .to_websocket_address();
        TypedSocketConnector::new(addr)
    }

    pub fn dns_connection(&self) -> TypedSocketConnector<MessageFromDns> {
        let url = self
            .controller_address
            .join("/ctrl/dns-socket")
            .to_websocket_address();
        TypedSocketConnector::new(url)
    }

    pub async fn connect(
        &self,
        cluster: &ClusterName,
        connect_request: &ConnectRequest,
    ) -> Result<ConnectResponse, PlaneClientError> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/c/{}/connect", cluster));

        let response = authed_post(&self.client, &addr, connect_request).await?;
        Ok(response)
    }

    pub async fn drain(
        &self,
        cluster: &ClusterName,
        drone: &DroneName,
    ) -> Result<(), PlaneClientError> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/c/{}/d/{}/drain", cluster, drone));

        authed_post(&self.client, &addr, &()).await?;
        Ok(())
    }

    pub async fn soft_terminate(
        &self,
        cluster: &ClusterName,
        backend_id: &BackendName,
    ) -> Result<(), PlaneClientError> {
        let addr = self.controller_address.join(&format!(
            "/ctrl/c/{}/b/{}/soft-terminate",
            cluster, backend_id
        ));

        authed_post(&self.client, &addr, &()).await?;
        Ok(())
    }

    pub async fn hard_terminate(
        &self,
        cluster: &ClusterName,
        backend_id: &BackendName,
    ) -> Result<(), PlaneClientError> {
        let addr = self.controller_address.join(&format!(
            "/ctrl/c/{}/b/{}/hard-terminate",
            cluster, backend_id
        ));

        authed_post(&self.client, &addr, &()).await?;
        Ok(())
    }

    pub fn backend_status_url(&self, cluster: &ClusterName, backend_id: &BackendName) -> Url {
        self.controller_address
            .join(&format!("/pub/c/{}/b/{}/status", cluster, backend_id))
            .url
    }

    pub async fn backend_status(
        &self,
        cluster: &ClusterName,
        backend_id: &BackendName,
    ) -> Result<TimestampedBackendStatus, PlaneClientError> {
        let url = self.backend_status_url(cluster, backend_id);

        let response = self.client.get(url).send().await?;
        let status: TimestampedBackendStatus = get_response(response).await?;
        Ok(status)
    }

    pub fn backend_status_stream_url(
        &self,
        cluster: &ClusterName,
        backend_id: &BackendName,
    ) -> Url {
        self.controller_address
            .join(&format!(
                "/pub/c/{}/b/{}/status-stream",
                cluster, backend_id
            ))
            .url
    }

    pub async fn backend_status_stream(
        &self,
        cluster: &ClusterName,
        backend_id: &BackendName,
    ) -> Result<sse::SseStream<TimestampedBackendStatus>, PlaneClientError> {
        let url = self.backend_status_stream_url(cluster, backend_id);

        let stream = sse::sse_request(url, self.client.clone()).await?;
        Ok(stream)
    }
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

async fn authed_get<T: DeserializeOwned>(
    client: &reqwest::Client,
    addr: &AuthorizedAddress,
) -> Result<T, PlaneClientError> {
    let mut req = client.get(addr.url.clone());
    if let Some(header) = addr.bearer_header() {
        req = req.header("Authorization", header);
    }

    let response = req.send().await?;
    get_response(response).await
}

async fn authed_post<T: DeserializeOwned>(
    client: &reqwest::Client,
    addr: &AuthorizedAddress,
    body: &impl serde::Serialize,
) -> Result<T, PlaneClientError> {
    let mut req = client.post(addr.url.clone());
    if let Some(header) = addr.bearer_header() {
        req = req.header("Authorization", header);
    }

    let response = req.json(body).send().await?;
    get_response(response).await
}
