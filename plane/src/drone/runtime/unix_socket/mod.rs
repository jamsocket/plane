use crate::{
    database::backend::BackendMetricsMessage,
    drone::runtime::{
        docker::{SpawnResult, TerminateEvent},
        Runtime,
    },
    names::BackendName,
    protocol::AcquiredKey,
    types::{backend_state::BackendError, BearerToken},
};
use anyhow::anyhow;
use anyhow::{Error, Result};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize)]
enum RuntimeCommand {
    Prepare {
        config: String,
    },
    Spawn {
        backend_id: String,
        executable: String,
        acquired_key: Option<String>,
        static_token: Option<String>,
    },
    Terminate {
        backend_id: String,
        hard: bool,
    },
    MetricsCallback,
    WaitForBackend {
        backend_id: String,
        address: String,
    },
}

#[derive(Serialize, Deserialize)]
enum RuntimeResponse {
    Ok,
    SpawnResponse(SpawnResult),
    Error(String),
}

impl From<RuntimeResponse> for Result<(), Error> {
    fn from(response: RuntimeResponse) -> Self {
        match response {
            RuntimeResponse::Ok => Ok(()),
            RuntimeResponse::Error(e) => Err(anyhow!(e.clone())),
            _ => Err(anyhow!("Unexpected response type")),
        }
    }
}

impl From<RuntimeResponse> for Result<(), BackendError> {
    fn from(response: RuntimeResponse) -> Self {
        match response {
            RuntimeResponse::Error(_) => Err(BackendError::StartupTimeout),
            _ => Ok(()),
        }
    }
}

impl From<RuntimeResponse> for Result<SpawnResult, Error> {
    fn from(response: RuntimeResponse) -> Self {
        match response {
            RuntimeResponse::SpawnResponse(result) => Ok(result.clone()),
            RuntimeResponse::Error(e) => Err(anyhow!(e.clone())),
            _ => Err(anyhow!("Unexpected response type")),
        }
    }
}

pub struct UnixSocketRuntimeConfig {
    socket_path: String,
}

pub struct UnixSocketRuntime {
    config: UnixSocketRuntimeConfig,
    stream: Arc<Mutex<Option<UnixStream>>>,
}

impl UnixSocketRuntime {
    pub async fn new(socket_path: String) -> Result<Self> {
        let stream = UnixStream::connect(&socket_path).await.ok();
        Ok(UnixSocketRuntime {
            config: UnixSocketRuntimeConfig { socket_path },
            stream: Arc::new(Mutex::new(stream)),
        })
    }

    async fn ensure_connected(&self) -> Result<()> {
        let mut stream = self.stream.lock().await;
        if stream.is_none() {
            *stream = Some(UnixStream::connect(&self.config.socket_path).await?);
        }
        Ok(())
    }

    async fn send_command(&self, command: RuntimeCommand) -> Result<RuntimeResponse, Error> {
        self.ensure_connected().await?;
        let mut stream = self.stream.lock().await;
        if let Some(ref mut stream) = *stream {
            let message = serde_json::to_vec(&command)?;
            stream.write_all(&message).await?;
            let mut response = vec![0; 1024];
            stream.read(&mut response).await?;
            let response: RuntimeResponse = serde_json::from_slice(&response)?;
            match response {
                RuntimeResponse::Error(e) => Err(anyhow!(e)),
                _ => Ok(response),
            }
        } else {
            Err(anyhow!("Failed to connect to Unix socket"))
        }
    }
}

impl Runtime for UnixSocketRuntime {
    type RuntimeConfig = ();
    type BackendConfig = String; // Simplified for example purposes

    async fn prepare(&self, config: &Self::BackendConfig) -> Result<(), Error> {
        self.send_command(RuntimeCommand::Prepare {
            config: config.clone(),
        })
        .await?
        .into()
    }

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: Self::BackendConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult, Error> {
        self.send_command(RuntimeCommand::Spawn {
            backend_id: backend_id.to_string(),
            executable,
            acquired_key: acquired_key.map(|k| k.to_string()),
            static_token: static_token.map(|t| t.to_string()),
        })
        .await?
        .into()
    }
    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<(), Error> {
        self.send_command(RuntimeCommand::Terminate {
            backend_id: backend_id.to_string(),
            hard,
        })
        .await?
        .into()
    }

    async fn metrics_callback<F: Fn(BackendMetricsMessage) + Send + Sync + 'static>(
        &self,
        sender: F,
    ) {
        // This method might need a different IPC mechanism to stream data continuously
        todo!()
    }

    fn events(&self) -> impl Stream<Item = TerminateEvent> + Send {
        // Stream is to iterator as what future is to T. Seems here we need to
        // receive terminate events from the runtime. We can do similar to
        // what's done in docker with BroadCaststream::new -- the only part
        // different here is we need a way to receive the terminating event over
        // the unix socket.
        todo!()
    }

    async fn wait_for_backend(
        &self,
        backend: &BackendName,
        address: SocketAddr,
    ) -> Result<(), BackendError> {
        self.send_command(RuntimeCommand::WaitForBackend {
            backend_id: backend.to_string(),
            address: address.to_string(),
        })
        .await
        .map_err(|_| BackendError::StartupTimeout)?
        .into()
    }
}
