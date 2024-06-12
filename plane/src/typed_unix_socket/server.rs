use super::{get_quick_backoff, WrappedMessage};
use anyhow::Error;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Debug,
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{UnixListener, UnixStream},
    sync::broadcast,
};

#[derive(Clone)]
pub struct TypedUnixSocketServer<MessageToServer, MessageToClient>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    socket_path: PathBuf,
    event_tx: broadcast::Sender<MessageToServer>,
    request_tx: broadcast::Sender<WrappedMessage<MessageToServer>>,
    response_tx: broadcast::Sender<WrappedMessage<MessageToClient>>,
}

impl<MessageToServer, MessageToClient> TypedUnixSocketServer<MessageToServer, MessageToClient>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    pub async fn new<P: AsRef<Path>>(socket_path: P) -> Result<Self, Error> {
        let socket_path = socket_path.as_ref().to_path_buf();
        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        let (event_tx, _) = broadcast::channel(100);
        let (request_tx, _) = broadcast::channel(100);
        let (response_tx, _) = broadcast::channel(100);

        tokio::spawn({
            let event_tx = event_tx.clone();
            let request_tx = request_tx.clone();
            let response_tx = response_tx.clone();
            async move {
                let mut backoff = get_quick_backoff();
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            tokio::spawn(handle_connection(
                                stream,
                                event_tx.clone(),
                                request_tx.clone(),
                                response_tx.subscribe(),
                            ));
                        }
                        Err(e) => {
                            tracing::error!("Error accepting connection: {}", e);
                            backoff.wait().await;
                        }
                    }
                }
            }
        });

        Ok(Self {
            socket_path,
            event_tx,
            request_tx,
            response_tx,
        })
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<MessageToServer> {
        self.event_tx.subscribe()
    }

    pub fn subscribe_requests(&self) -> broadcast::Receiver<WrappedMessage<MessageToServer>> {
        self.request_tx.subscribe()
    }

    pub async fn send_response(&self, id: String, response: MessageToClient) -> Result<(), Error> {
        // Wait until there is at least one subscriber
        let mut backoff = get_quick_backoff();
        while self.response_tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            backoff.wait().await;
        }

        let response = WrappedMessage {
            id: Some(id),
            message: response,
        };

        self.response_tx.send(response)?;
        Ok(())
    }

    pub async fn send_message(&self, message: MessageToClient) -> Result<(), Error> {
        let message_msg = WrappedMessage {
            id: None,
            message: message,
        };

        // Wait until there is at least one subscriber
        let mut backoff = get_quick_backoff();
        while self.response_tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            backoff.wait().await;
        }

        self.response_tx.send(message_msg)?;
        Ok(())
    }
}

impl<MessageToServer, MessageToClient> Drop
    for TypedUnixSocketServer<MessageToServer, MessageToClient>
where
    MessageToServer: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    MessageToClient: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.socket_path) {
            tracing::warn!("Failed to remove socket file: {}", e);
        }
    }
}

async fn handle_connection<MessageToServer, MessageToClient>(
    stream: UnixStream,
    event_tx: broadcast::Sender<MessageToServer>,
    request_tx: broadcast::Sender<WrappedMessage<MessageToServer>>,
    mut response_rx: broadcast::Receiver<WrappedMessage<MessageToClient>>,
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
        async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let msg: WrappedMessage<MessageToServer> = match serde_json::from_str(&line)
                        {
                            Ok(msg) => msg,
                            Err(e) => {
                                tracing::error!("Error deserializing message: {}", e);
                                continue;
                            }
                        };
                        match msg {
                            WrappedMessage { id: Some(_), .. } => {
                                if let Err(e) = request_tx.send(msg) {
                                    tracing::error!("Error sending request: {}", e);
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
                        tracing::error!("Connection closed by client");
                    }
                    Err(e) => {
                        tracing::error!("Error reading line: {}", e);
                    }
                }
            }
        }
    });

    // Task to handle sending responses
    let send_task = tokio::spawn(async move {
        loop {
            match response_rx.recv().await {
                Ok(response) => {
                    let response_str = match serde_json::to_string(&response) {
                        Ok(response_str) => response_str,
                        Err(e) => {
                            tracing::error!("Error serializing response: {}", e);
                            continue;
                        }
                    };
                    if let Err(e) = writer.write_all(response_str.as_bytes()).await {
                        tracing::error!("Error writing response: {}", e);
                    }
                    if let Err(e) = writer.write_all(b"\n").await {
                        tracing::error!("Error writing newline: {}", e);
                    }
                    if let Err(e) = writer.flush().await {
                        tracing::error!("Error flushing writer: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Error receiving response: {}", e);
                }
            }
        }
    });

    let _ = tokio::try_join!(recv_task, send_task);

    Ok(())
}
