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
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, sync::{mpsc, oneshot}};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_serde::formats::Json;
use tokio_serde::Framed as SerdeFramed;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
enum RuntimeCommand {
    Prepare {
        id: Uuid,
        config: String,
    },
    Spawn {
        id: Uuid,
        backend_id: String,
        executable: String,
        acquired_key: Option<String>,
        static_token: Option<String>,
    },
    Terminate {
        id: Uuid,
        backend_id: String,
        hard: bool,
    },
    MetricsCallback {
        id: Uuid,
    },
    WaitForBackend {
        id: Uuid,
        backend_id: String,
        address: String,
    },
}

#[derive(Serialize, Deserialize)]
enum RuntimeResponse {
    Ok {
        id: Uuid,
    },
    SpawnResponse {
        id: Uuid,
        result: SpawnResult,
    },
    Error {
        id: Uuid,
        message: String,
    },
}

impl From<RuntimeResponse> for Result<(), Error> {
    fn from(response: RuntimeResponse) -> Self {
        match response {
            RuntimeResponse::Ok { .. } => Ok(()),
            RuntimeResponse::Error { message, .. } => Err(anyhow!(message.clone())),
            _ => Err(anyhow!("Unexpected response type")),
        }
    }
}

impl From<RuntimeResponse> for Result<(), BackendError> {
    fn from(response: RuntimeResponse) -> Self {
        match response {
            RuntimeResponse::Error { .. } => Err(BackendError::StartupTimeout),
            _ => Ok(()),
        }
    }
}

impl From<RuntimeResponse> for Result<SpawnResult, Error> {
    fn from(response: RuntimeResponse) -> Self {
        match response {
            RuntimeResponse::SpawnResponse { result, .. } => Ok(result.clone()),
            RuntimeResponse::Error { message, .. } => Err(anyhow!(message.clone())),
            _ => Err(anyhow!("Unexpected response type")),
        }
    }
}

pub struct UnixSocketRuntimeConfig {
    socket_path: String,
}

pub struct UnixSocketRuntime {
    config: UnixSocketRuntimeConfig,
    tx: mpsc::Sender<RuntimeCommand>,
    rx: Arc<Mutex<mpsc::Receiver<RuntimeResponse>>>,
}

impl UnixSocketRuntime {
    pub async fn new(socket_path: String) -> Result<Self> {
        let stream = UnixStream::connect(&socket_path).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let (response_tx, response_rx) = mpsc::unbounded_channel();

        tokio::spawn(handle_connection(stream, rx, response_tx));

        Ok(UnixSocketRuntime {
            config: UnixSocketRuntimeConfig { socket_path },
            tx,
            rx: Arc::new(Mutex::new(response_rx)),
        })
    }

    async fn send_command(&self, command: RuntimeCommand) -> Result<RuntimeResponse, Error> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx.send(command).await?;
        let response = response_rx.await?;
        Ok(response)
    }

    async fn wait_for_response(&self, id: Uuid) -> Result<RuntimeResponse, Error> {
        let mut rx = self.rx.lock().await;
        while let Some(response) = rx.recv().await {
            match response {
                RuntimeResponse::Ok { id: resp_id }
                | RuntimeResponse::SpawnResponse { id: resp_id, .. }
                | RuntimeResponse::Error { id: resp_id, .. } if resp_id == id => {
                    return Ok(response);
                }
                _ => continue,
            }
        }
        Err(anyhow!("Failed to receive response"))
    }
}

async fn handle_connection(
    stream: UnixStream,
    mut rx: mpsc::Receiver<RuntimeCommand>,
    response_tx: mpsc::Sender<RuntimeResponse>,
) -> Result<(), Box<dyn Error>> {
    let length_delimited = Framed::new(stream, LengthDelimitedCodec::new());
    let mut framed = SerdeFramed::new(length_delimited, Json::<RuntimeCommand, RuntimeResponse>::default());

    // Task to handle receiving messages
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = framed.try_next().await {
            if response_tx.send(msg).await.is_err() {
                eprintln!("failed to send response");
                return;
            }
        }
    });

    // Task to handle sending messages
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if framed.send(msg).await.is_err() {
                eprintln!("failed to send message");
                return;
            }
        }
    });

    tokio::try_join!(recv_task, send_task)?;
    Ok(())
}

impl Runtime for UnixSocketRuntime {
    type RuntimeConfig = ();
    type BackendConfig = String; // Simplified for example purposes

    async fn prepare(&self, config: &Self::BackendConfig) -> Result<(), Error> {
        let id = Uuid::new_v4();
        self.tx.send(RuntimeCommand::Prepare {
            id,
            config: config.clone(),
        }).await?;
        self.wait_for_response(id).await?.into()
    }

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: Self::BackendConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult, Error> {
        let id = Uuid::new_v4();
        self.tx.send(RuntimeCommand::Spawn {
            id,
            backend_id: backend_id.to_string(),
            executable,
            acquired_key: acquired_key.map(|k| k.to_string()),
            static_token: static_token.map(|t| t.to_string()),
        }).await?;
        self.wait_for_response(id).await?.into()
    }

    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<(), Error> {
        let id = Uuid::new_v4();
        self.tx.send(RuntimeCommand::Terminate {
            id,
            backend_id: backend_id.to_string(),
            hard,
        }).await?;
        self.wait_for_response(id).await?.into()
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
        let id = Uuid::new_v4();
        self.tx.send(RuntimeCommand::WaitForBackend {
            id,
            backend_id: backend.to_string(),
            address: address.to_string(),
        }).await?;
        self.wait_for_response(id).await?.into()
    }
}
