use super::{
    docker::{SpawnResult, TerminateEvent},
    Runtime,
};
use crate::{
    database::backend::BackendMetricsMessage,
    names::BackendName,
    protocol::AcquiredKey,
    typed_unix_socket::client::TypedUnixSocketClient,
    types::{backend_state::BackendError, BearerToken, DockerExecutorConfig},
};
use anyhow::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf, pin::Pin};
use tokio_stream::{Stream, StreamExt};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MessageToServer {
    Prepare(DockerExecutorConfig),
    Spawn(
        BackendName,
        DockerExecutorConfig,
        Option<AcquiredKey>,
        Option<BearerToken>,
    ),
    Terminate(BackendName, bool),
    WaitForBackend(BackendName, SocketAddr),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageToClient {
    PrepareResult(Result<(), String>),
    SpawnResult(Result<SpawnResult, String>),
    TerminateResult(Result<bool, String>),
    WaitForBackendResult(Result<(), BackendError>),
    MetricsMessage(BackendMetricsMessage),
    TerminateEvent(TerminateEvent),
}

pub struct UnixSocketRuntime {
    client: TypedUnixSocketClient<MessageToServer, MessageToClient>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnixSocketRuntimeConfig {
    pub socket_path: PathBuf,
}

#[async_trait::async_trait]
impl Runtime for UnixSocketRuntime {
    async fn prepare(&self, config: &serde_json::Value) -> Result<()> {
        let config: DockerExecutorConfig = serde_json::from_value(config.clone())?;

        let response = self
            .client
            .send_request(MessageToServer::Prepare(config.clone()))
            .await?;
        match response {
            MessageToClient::PrepareResult(Ok(())) => Ok(()),
            MessageToClient::PrepareResult(Err(e)) => Err(Error::msg(e)),
            _ => Err(Error::msg("Unexpected response from server")),
        }
    }

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: &serde_json::Value,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult> {
        let executable: DockerExecutorConfig = serde_json::from_value(executable.clone())?;

        let response = self
            .client
            .send_request(MessageToServer::Spawn(
                backend_id.clone(),
                executable,
                acquired_key.cloned(),
                static_token.cloned(),
            ))
            .await?;
        match response {
            MessageToClient::SpawnResult(Ok(result)) => Ok(result),
            MessageToClient::SpawnResult(Err(e)) => Err(Error::msg(e)),
            _ => Err(Error::msg("Unexpected response from server")),
        }
    }

    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<bool> {
        let response = self
            .client
            .send_request(MessageToServer::Terminate(backend_id.clone(), hard))
            .await?;
        match response {
            MessageToClient::TerminateResult(Ok(result)) => Ok(result),
            MessageToClient::TerminateResult(Err(e)) => Err(Error::msg(e)),
            _ => Err(Error::msg("Unexpected response from server")),
        }
    }

    fn events(&self) -> Pin<Box<dyn Stream<Item = TerminateEvent> + Send>> {
        let mut event_rx = self.client.subscribe_events();
        Box::pin(async_stream::stream! {
            while let Ok(event) = event_rx.recv().await {
                if let MessageToClient::TerminateEvent(terminate_event) = event {
                    yield terminate_event;
                }
            }
        })
    }

    fn metrics_callback(&self, sender: Box<dyn Fn(BackendMetricsMessage) + Send + Sync + 'static>) {
        let mut event_rx = self.client.subscribe_events();
        tokio::spawn(async move {
            while let Ok(event) = event_rx.recv().await {
                if let MessageToClient::MetricsMessage(metrics) = event {
                    sender(metrics);
                }
            }
        });
    }

    async fn wait_for_backend(
        &self,
        backend: &BackendName,
        address: SocketAddr,
    ) -> Result<(), BackendError> {
        let response = self
            .client
            .send_request(MessageToServer::WaitForBackend(backend.clone(), address))
            .await
            .expect("Failed to send request");
        match response {
            MessageToClient::WaitForBackendResult(v) => v,
            _ => Err(BackendError::Other(
                "Unexpected response from server".to_string(),
            )),
        }
    }
}

impl UnixSocketRuntime {
    pub async fn new(config: UnixSocketRuntimeConfig) -> Result<Self> {
        let client: TypedUnixSocketClient<MessageToServer, MessageToClient> =
            TypedUnixSocketClient::new(&config.socket_path).await?;
        Ok(Self { client })
    }
}
