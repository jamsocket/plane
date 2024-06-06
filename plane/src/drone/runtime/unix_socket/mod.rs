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
use dashmap::DashMap;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{broadcast, mpsc, oneshot},
};
use tokio_serde::formats::Json;
use tokio_serde::Framed as SerdeFramed;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct CommandWrapper {
    id: Uuid,
    command: RuntimeCommand,
}

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
struct ResponseWrapper {
    id: Uuid,
    response: RuntimeResponse,
}

#[derive(Serialize, Deserialize)]
enum RuntimeResponse {
    Ok,
    SpawnResponse { result: SpawnResult },
    Error { message: String },
    TerminateEvent(TerminateEvent),
}

impl From<ResponseWrapper> for Result<(), Error> {
    fn from(wrapper: ResponseWrapper) -> Self {
        match wrapper.response {
            RuntimeResponse::Ok => Ok(()),
            RuntimeResponse::Error { message } => Err(anyhow!(message.clone())),
            _ => Err(anyhow!("Unexpected response type")),
        }
    }
}

impl From<ResponseWrapper> for Result<(), BackendError> {
    fn from(wrapper: ResponseWrapper) -> Self {
        match wrapper.response {
            RuntimeResponse::Error { .. } => Err(BackendError::StartupTimeout),
            _ => Ok(()),
        }
    }
}

impl From<ResponseWrapper> for Result<SpawnResult, Error> {
    fn from(wrapper: ResponseWrapper) -> Self {
        match wrapper.response {
            RuntimeResponse::SpawnResponse { result } => Ok(result.clone()),
            RuntimeResponse::Error { message } => Err(anyhow!(message.clone())),
            _ => Err(anyhow!("Unexpected response type")),
        }
    }
}

pub struct UnixSocketRuntimeConfig {
    socket_path: String,
}

pub struct UnixSocketRuntime {
    config: UnixSocketRuntimeConfig,
    tx: mpsc::Sender<CommandWrapper>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseWrapper>>>,
    event_tx: broadcast::Sender<TerminateEvent>,
}

impl UnixSocketRuntime {
    pub async fn new(socket_path: String) -> Result<Self> {
        let stream = UnixStream::connect(&socket_path).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let response_map = Arc::new(DashMap::new());
        let (event_tx, _) = broadcast::channel(100);

        tokio::spawn(handle_connection(
            stream,
            rx,
            Arc::clone(&response_map),
            event_tx.clone(),
        ));

        Ok(UnixSocketRuntime {
            config: UnixSocketRuntimeConfig { socket_path },
            tx,
            response_map,
            event_tx,
        })
    }

    async fn send_command(&self, command: RuntimeCommand) -> Result<RuntimeResponse, Error> {
        let (response_tx, response_rx) = oneshot::channel();
        let id = Uuid::new_v4();
        let wrapper = CommandWrapper { id, command };

        self.response_map.insert(id, response_tx);
        self.tx.send(wrapper).await?;
        let response = response_rx.await?;
        Ok(response.response)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<TerminateEvent> {
        self.event_tx.subscribe()
    }
}

async fn handle_connection(
    stream: UnixStream,
    mut rx: mpsc::Receiver<CommandWrapper>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseWrapper>>>,
    event_tx: broadcast::Sender<TerminateEvent>,
) -> Result<(), Box<dyn Error>> {
    let length_delimited = Framed::new(stream, LengthDelimitedCodec::new());
    let mut framed = SerdeFramed::new(
        length_delimited,
        Json::<CommandWrapper, ResponseWrapper>::default(),
    );

    // Task to handle receiving messages
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = framed.try_next().await {
            let id = msg.id;

            match msg.response {
                RuntimeResponse::TerminateEvent(event) => {
                    let _ = event_tx.send(event);
                }
                _ => {
                    if let Some(tx) = response_map.remove(&id).map(|(_, tx)| tx) {
                        let _ = tx.send(msg);
                    } else {
                        eprintln!("No sender found for response ID: {:?}", id);
                    }
                }
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
        let command = RuntimeCommand::Prepare {
            config: config.clone(),
        };
        self.send_command(command).await?.into()
    }

    async fn spawn(
        &self,
        backend_id: &BackendName,
        executable: Self::BackendConfig,
        acquired_key: Option<&AcquiredKey>,
        static_token: Option<&BearerToken>,
    ) -> Result<SpawnResult, Error> {
        let command = RuntimeCommand::Spawn {
            backend_id: backend_id.to_string(),
            executable,
            acquired_key: acquired_key.map(|k| k.to_string()),
            static_token: static_token.map(|t| t.to_string()),
        };
        self.send_command(command).await?.into()
    }

    async fn terminate(&self, backend_id: &BackendName, hard: bool) -> Result<(), Error> {
        let command = RuntimeCommand::Terminate {
            backend_id: backend_id.to_string(),
            hard,
        };
        self.send_command(command).await?.into()
    }

    async fn metrics_callback<F: Fn(BackendMetricsMessage) + Send + Sync + 'static>(
        &self,
        sender: F,
    ) {
        // This method might need a different IPC mechanism to stream data continuously
        todo!()
    }

    fn events(&self) -> impl Stream<Item = TerminateEvent> + Send {
        BroadcastStream::new(self.subscribe_events()).filter_map(|result| async {
            match result {
                Ok(event) => Some(event),
                Err(_) => None, // Handle lagging or closed receiver
            }
        })
    }

    async fn wait_for_backend(
        &self,
        backend: &BackendName,
        address: SocketAddr,
    ) -> Result<(), BackendError> {
        let command = RuntimeCommand::WaitForBackend {
            backend_id: backend.to_string(),
            address: address.to_string(),
        };
        self.send_command(command).await?.into()
    }
}
