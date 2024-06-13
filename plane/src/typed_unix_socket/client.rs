use super::{get_quick_backoff, WrappedMessage};
use crate::util::random_token;
use anyhow::Error;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{clone::Clone, fmt::Debug, path::Path, sync::Arc};
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader, BufWriter};
use tokio::{
    net::UnixStream,
    sync::{broadcast, oneshot, watch},
};

#[derive(Clone)]
pub struct TypedUnixSocketClient<MessageToServer, MessageToClient>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    tx: broadcast::Sender<WrappedMessage<MessageToServer>>,
    response_map: Arc<DashMap<String, oneshot::Sender<MessageToClient>>>,
    event_tx: broadcast::Sender<MessageToClient>,
    shutdown_tx: watch::Sender<()>,
}

impl<MessageToServer, MessageToClient> TypedUnixSocketClient<MessageToServer, MessageToClient>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    pub async fn new<P: AsRef<Path>>(socket_path: P) -> Result<Self, Error> {
        let (tx, _) = broadcast::channel(100);
        let response_map = Arc::new(DashMap::new());
        let (event_tx, _) = broadcast::channel(100);
        let (shutdown_tx, shutdown_rx) = watch::channel(());

        tokio::spawn({
            let socket_path = socket_path.as_ref().to_path_buf();
            let tx = tx.clone();
            let response_map = Arc::clone(&response_map);
            let event_tx = event_tx.clone();
            let mut shutdown_rx = shutdown_rx.clone();
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
                                shutdown_rx.clone(),
                            )
                            .await
                            .unwrap_or_else(|e| {
                                tracing::error!("Error handling connection: {}", e);
                            });
                            break;
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
            shutdown_tx,
        })
    }

    pub async fn send_request(&self, request: MessageToServer) -> Result<MessageToClient, Error> {
        let (response_tx, response_rx) = oneshot::channel();
        let id = random_token();

        let wrapper = WrappedMessage {
            id: Some(id.clone()),
            message: request,
        };

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

    pub async fn send_message(&self, message: MessageToServer) -> Result<(), Error> {
        let wrapper = WrappedMessage { id: None, message };

        // Wait until there is at least one subscriber
        let mut backoff = get_quick_backoff();
        while self.tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            backoff.wait().await;
        }

        self.tx.send(wrapper)?;
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<MessageToClient> {
        self.event_tx.subscribe()
    }
}

impl<MessageToServer, MessageToClient> Drop
    for TypedUnixSocketClient<MessageToServer, MessageToClient>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

async fn handle_connection<MessageToServer, MessageToClient>(
    stream: UnixStream,
    mut rx: broadcast::Receiver<WrappedMessage<MessageToServer>>,
    response_map: Arc<DashMap<String, oneshot::Sender<MessageToClient>>>,
    event_tx: broadcast::Sender<MessageToClient>,
    shutdown_rx: watch::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
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
        let mut shutdown_rx = shutdown_rx.clone();
        async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Shutting down receive task");
                        break;
                    }
                    result = lines.next_line() => {
                        match result {
                            Ok(Some(line)) => {
                                let msg: WrappedMessage<MessageToClient> = match serde_json::from_str(&line)
                                {
                                    Ok(msg) => msg,
                                    Err(e) => {
                                        tracing::error!("Error deserializing message: {}", e);
                                        continue;
                                    }
                                };
                                match msg {
                                    WrappedMessage {
                                        id: Some(id),
                                        message,
                                    } => {
                                        if let Some((_, tx)) = response_map.remove(&id) {
                                            if let Err(e) = tx.send(message) {
                                                tracing::error!("Error sending response: {:?}", e);
                                            }
                                        } else {
                                            tracing::error!("No sender found for response ID: {:?}", id);
                                        }
                                    }
                                    WrappedMessage { id: None, message } => {
                                        if let Err(e) = event_tx.send(message) {
                                            tracing::error!("Error sending event: {}", e);
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
            }
        }
    });

    // Task to handle sending messages
    let send_task = tokio::spawn({
        let mut shutdown_rx = shutdown_rx.clone();
        async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Shutting down send task");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
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
                }
            }
        }
    });

    tokio::join!(recv_task, send_task);

    Ok(())
}
