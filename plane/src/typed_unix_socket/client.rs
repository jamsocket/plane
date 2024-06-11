use super::{IDedMessage, WrappedClientMessageType, WrappedServerMessageType};
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
    shutdown_tx: watch::Sender<()>,
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
        let (shutdown_tx, shutdown_rx) = watch::channel(());

        // Handle ctrl_c signal
        tokio::spawn({
            let shutdown_tx = shutdown_tx.clone();
            async move {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to install Ctrl+C handler");
                tracing::info!("Ctrl+C received, initiating shutdown...");
                let _ = shutdown_tx.send(());
            }
        });

        let stream = loop {
            match UnixStream::connect(&socket_path).await {
                Ok(stream) => break stream,
                Err(e) => {
                    tracing::error!("Error connecting to server: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        };

        tokio::spawn({
            let tx = tx.clone();
            let response_map = Arc::clone(&response_map);
            let event_tx = event_tx.clone();
            let shutdown_rx = shutdown_rx.clone();
            async move {
                handle_connection(stream, tx.subscribe(), response_map, event_tx, shutdown_rx)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!("Error handling connection: {}", e);
                    });
            }
        });

        Ok(Self {
            tx,
            response_map,
            event_tx,
            shutdown_tx,
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
        while self.tx.receiver_count() == 0 {
            println!("wait loop");
            tracing::info!("Waiting for a subscriber...");
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        self.tx.send(wrapper)?;
        println!("sent");
        let response = response_rx.await?;
        println!("got response");
        Ok(response)
    }

    pub async fn send_message(&self, message: ClientMessageType) -> Result<(), Error> {
        let wrapper = WrappedClientMessageType::ClientMessage(message);

        // Wait until there is at least one subscriber
        while self.tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        self.tx.send(wrapper)?;
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerMessageType> {
        self.event_tx.subscribe()
    }
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType> Drop
    for TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    fn drop(&mut self) {
        if let Err(e) = self.shutdown_tx.send(()) {
            tracing::warn!("Failed to send shutdown signal: {}", e);
        }
    }
}

async fn handle_connection<ClientMessageType, ServerMessageType, RequestType, ResponseType>(
    stream: UnixStream,
    mut rx: broadcast::Receiver<WrappedClientMessageType<RequestType, ClientMessageType>>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseType>>>,
    event_tx: broadcast::Sender<ServerMessageType>,
    shutdown_rx: watch::Receiver<()>,
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
        let mut shutdown_rx = shutdown_rx.clone();
        async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Shutdown signal received in recv_task.");
                        break;
                    }
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                match serde_json::from_str::<WrappedServerMessageType<ResponseType, ServerMessageType>>(&line) {
                                    Ok(msg) => {
                                        match msg {
                                            WrappedServerMessageType::ServerMessage(event) => {
                                                if let Err(e) = event_tx.send(event) {
                                                    tracing::error!("Failed to send server message: {:?}", e);
                                                }
                                            }
                                            WrappedServerMessageType::Response(response) => {
                                                let id = response.id;
                                                match response_map.remove(&id).map(|(_, tx)| tx) {
                                                    Some(tx) => {
                                                        if let Err(e) = tx.send(response.message) {
                                                            tracing::error!("Failed to send response message: {:?}", e);
                                                        }
                                                    }
                                                    None => {
                                                        tracing::error!("No sender found for response ID: {:?}", id);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to deserialize message: {:?}", e);
                                    }
                                }
                            }
                            Ok(None) => {
                                // End of stream, no more lines to read
                                tracing::warn!("Encountered end of stream");
                            }
                            Err(e) => {
                                tracing::error!("Failed to read next line: {:?}", e);
                            }
                        }
                    }
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    });

    // Task to handle sending messages
    let send_task = tokio::spawn({
        let mut shutdown_rx = shutdown_rx.clone(); // Make shutdown_rx mutable
        async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Shutdown signal received in send_task.");
                        break;
                    }
                    msg = rx.recv() => {
                        match msg {
                            Ok(msg) => {
                                match serde_json::to_string(&msg) {
                                    Ok(msg) => {
                                        if let Err(e) = writer.write_all(msg.as_bytes()).await {
                                            tracing::error!("Failed to write message: {:?}", e);
                                        }
                                        if let Err(e) = writer.write_all(b"\n").await {
                                            tracing::error!("Failed to write newline: {:?}", e);
                                        }
                                        if let Err(e) = writer.flush().await {
                                            tracing::error!("Failed to flush writer: {:?}", e);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to serialize message: {:?}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to receive message: {:?}", e);
                            }
                        }
                    }
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    });

    let (recv_result, send_result) = tokio::try_join!(recv_task, send_task)?;

    recv_result?;
    send_result?;
    Ok(())
}
