use self::controller_address::AuthorizedAddress;
use crate::{
    names::{BackendName, DroneName},
    protocol::{MessageFromDns, MessageFromDrone, MessageFromProxy},
    typed_socket::client::TypedSocketConnector,
    types::{
        backend_state::BackendStatusStreamEntry, ClusterName, ClusterState, ConnectRequest,
        ConnectResponse, DrainResult, DronePoolName, RevokeRequest,
    },
};
use protocol::{ApiError, StatusResponse};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;
use url::{form_urlencoded, Url};

pub mod controller_address;
pub mod exponential_backoff;
pub mod log_types;
pub mod names;
pub mod protocol;
pub mod serialization;
pub mod sse;
pub mod typed_socket;
pub mod types;
pub mod util;
pub mod version;

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

    #[error("Failed to connect.")]
    ConnectFailed(&'static str),

    #[error("Bad configuration.")]
    BadConfiguration(&'static str),

    #[error("WebSocket error: {0}")]
    Tungstenite(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("Send error")]
    SendFailed,
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
        pool: &DronePoolName,
    ) -> TypedSocketConnector<MessageFromDrone> {
        let base_path = format!("/ctrl/c/{}/drone-socket", cluster);
        let addr = if pool.is_default() {
            self.controller_address.join(&base_path)
        } else {
            let encoded_pool: String =
                form_urlencoded::byte_serialize(pool.as_str().as_bytes()).collect();
            self.controller_address
                .join(&format!("{}?pool={}", base_path, encoded_pool))
        }
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
        connect_request: &ConnectRequest,
    ) -> Result<ConnectResponse, PlaneClientError> {
        let addr = self.controller_address.join("/ctrl/connect");

        let response = authed_post(&self.client, &addr, connect_request).await?;
        Ok(response)
    }

    pub async fn drain(
        &self,
        cluster: &ClusterName,
        drone: &DroneName,
    ) -> Result<DrainResult, PlaneClientError> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/c/{}/d/{}/drain", cluster, drone));

        let result: DrainResult = authed_post(&self.client, &addr, &()).await?;
        Ok(result)
    }
    pub async fn soft_terminate(&self, backend_id: &BackendName) -> Result<(), PlaneClientError> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/b/{}/soft-terminate", backend_id));

        let _: () = authed_post(&self.client, &addr, &()).await?;
        Ok(())
    }

    pub async fn hard_terminate(&self, backend_id: &BackendName) -> Result<(), PlaneClientError> {
        let addr = self
            .controller_address
            .join(&format!("/ctrl/b/{}/hard-terminate", backend_id));

        let _: () = authed_post(&self.client, &addr, &()).await?;
        Ok(())
    }

    pub async fn revoke(&self, request: &RevokeRequest) -> Result<(), PlaneClientError> {
        let addr = self.controller_address.join("/ctrl/revoke");

        let _: () = authed_post(&self.client, &addr, &request).await?;
        Ok(())
    }

    pub fn backend_status_url(&self, backend_id: &BackendName) -> Url {
        self.controller_address
            .join(&format!("/pub/b/{}/status", backend_id))
            .url
    }

    pub async fn backend_status(
        &self,
        backend_id: &BackendName,
    ) -> Result<BackendStatusStreamEntry, PlaneClientError> {
        let url = self.backend_status_url(backend_id);

        let response = self.client.get(url).send().await?;
        let status: BackendStatusStreamEntry = get_response(response).await?;
        Ok(status)
    }

    pub fn backend_status_stream_url(&self, backend_id: &BackendName) -> Url {
        self.controller_address
            .join(&format!("/pub/b/{}/status-stream", backend_id))
            .url
    }

    pub async fn backend_status_stream(
        &self,
        backend_id: &BackendName,
    ) -> Result<sse::SseStream<BackendStatusStreamEntry>, PlaneClientError> {
        let url = self.backend_status_stream_url(backend_id);

        let stream = sse::sse_request(url, self.client.clone()).await?;
        Ok(stream)
    }

    pub async fn cluster_state(
        &self,
        cluster: &ClusterName,
    ) -> Result<ClusterState, PlaneClientError> {
        let url = self
            .controller_address
            .join(&format!("/ctrl/c/{}/state", cluster));
        let cluster_state: ClusterState = authed_get(&self.client, &url).await?;
        Ok(cluster_state)
    }

    pub async fn health_check(&self) -> Result<(), PlaneClientError> {
        let url = self.controller_address.join("/pub/health");
        self.client.get(url.url).send().await?;
        Ok(())
    }
}

async fn get_response<T: DeserializeOwned>(response: Response) -> Result<T, PlaneClientError> {
    if response.status().is_success() {
        Ok(response.json::<T>().await?)
    } else {
        let url = response.url().to_string();
        let status = response.status();
        if status.is_server_error() {
            tracing::error!(?url, ?status, "Got 5xx response from Plane API server.");
        } else {
            tracing::warn!(
                ?url,
                ?status,
                "Got unsuccessful response from Plane API server."
            );
        }
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
