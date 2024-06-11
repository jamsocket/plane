use super::{get_quick_backoff, IDedMessage, WrappedClientMessageType, WrappedServerMessageType};
use anyhow::Error;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{clone::Clone, fmt::Debug, path::Path, sync::Arc};
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader, BufWriter};
use tokio::{
    net::UnixStream,
    sync::{broadcast, oneshot},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    tx: broadcast::Sender<WrappedClientMessageType<RequestType, ClientMessageType>>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseType>>>,
    event_tx: broadcast::Sender<ServerMessageType>,
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType>
    TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    pub async fn new<P: AsRef<Path>>(socket_path: P) -> Result<Self, Error> {
        let (tx, _) = broadcast::channel(100);
        let response_map = Arc::new(DashMap::new());
        let (event_tx, _) = broadcast::channel(100);

        tokio::spawn({
            let socket_path = socket_path.as_ref().to_path_buf();
            let tx = tx.clone();
            let response_map = Arc::clone(&response_map);
            let event_tx = event_tx.clone();
            async move {
                let mut backoff = get_quick_backoff();
                loop {
                    match UnixStream::connect(&socket_path).await {
                        Ok(stream) => {
                            handle_connection(
                                stream,
                                tx.subscribe(),
                                Arc::clone(&response_map),
                                event_tx.clone(),
                            )
                            .await
                            .unwrap_or_else(|e| {
                                tracing::error!("Error handling connection: {}", e);
                            });
                        }
                        Err(e) => {
                            tracing::error!("Error connecting to server: {}", e);
                            backoff.wait().await;
                        }
                    }
                }
            }
        });
        Ok(Self {
            tx,
            response_map,
            event_tx,
        })
    }

    pub async fn send_request(&self, request: RequestType) -> Result<ResponseType, Error> {
        let (response_tx, response_rx) = oneshot::channel();
        let id = Uuid::new_v4();
        let wrapper = WrappedClientMessageType::Request(IDedMessage {
            id,
            message: request,
        });

        self.response_map.insert(id, response_tx);

        // Wait until there is at least one subscriber
        let mut backoff = get_quick_backoff();
        while self.tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            backoff.wait().await;
        }

        self.tx.send(wrapper)?;
        let response = response_rx.await?;
        Ok(response)
    }

    pub async fn send_message(&self, message: ClientMessageType) -> Result<(), Error> {
        let wrapper = WrappedClientMessageType::ClientMessage(message);

        // Wait until there is at least one subscriber
        let mut backoff = get_quick_backoff();
        while self.tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            backoff.wait().await;
        }

        self.tx.send(wrapper)?;
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerMessageType> {
        self.event_tx.subscribe()
    }
}

async fn handle_connection<ClientMessageType, ServerMessageType, RequestType, ResponseType>(
    stream: UnixStream,
    mut rx: broadcast::Receiver<WrappedClientMessageType<RequestType, ClientMessageType>>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseType>>>,
    event_tx: broadcast::Sender<ServerMessageType>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    let (read_half, write_half) = tokio::io::split(stream);
    let reader = BufReader::new(read_half);
    let writer = BufWriter::new(write_half);

    let mut lines = reader.lines();
    let mut writer = writer;

    // Task to handle receiving messages
    let recv_task = tokio::spawn({
        let event_tx = event_tx.clone();
        let response_map = Arc::clone(&response_map);
        async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let msg: WrappedServerMessageType<ResponseType, ServerMessageType> =
                            match serde_json::from_str(&line) {
                                Ok(msg) => msg,
                                Err(e) => {
                                    tracing::error!("Error deserializing message: {}", e);
                                    continue;
                                }
                            };
                        match msg {
                            WrappedServerMessageType::ServerMessage(event) => {
                                if let Err(e) = event_tx.send(event) {
                                    tracing::error!("Error sending event: {}", e);
                                }
                            }
                            WrappedServerMessageType::Response(response) => {
                                let id = response.id;
                                if let Some(tx) = response_map.remove(&id).map(|(_, tx)| tx) {
                                    if tx.send(response.message).is_err() {
                                        tracing::error!("Error sending response");
                                    }
                                } else {
                                    tracing::error!("No sender found for response ID: {:?}", id);
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::error!("Connection closed by server");
                    }
                    Err(e) => {
                        tracing::error!("Error reading line: {}", e);
                    }
                }
            }
        }
    });

    // Task to handle sending messages
    let send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    let msg = match serde_json::to_string(&msg) {
                        Ok(msg) => msg,
                        Err(e) => {
                            tracing::error!("Error serializing message: {}", e);
                            continue;
                        }
                    };
                    if let Err(e) = writer.write_all(msg.as_bytes()).await {
                        tracing::error!("Error writing message: {}", e);
                    }
                    if let Err(e) = writer.write_all(b"\n").await {
                        tracing::error!("Error writing newline: {}", e);
                    }
                    if let Err(e) = writer.flush().await {
                        tracing::error!("Error flushing writer: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Error receiving message: {}", e);
                }
            }
        }
    });

    let _ = tokio::try_join!(recv_task, send_task);

    Ok(())
}
