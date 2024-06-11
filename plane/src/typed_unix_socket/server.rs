use super::{get_quick_backoff, IDedMessage, WrappedClientMessageType, WrappedServerMessageType};
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
pub struct TypedUnixSocketServer<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    socket_path: PathBuf,
    event_tx: broadcast::Sender<ClientMessageType>,
    request_tx: broadcast::Sender<IDedMessage<RequestType>>,
    response_tx: broadcast::Sender<WrappedServerMessageType<ResponseType, ServerMessageType>>,
    _phantom: PhantomData<(ServerMessageType, ResponseType)>,
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType>
    TypedUnixSocketServer<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
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
            _phantom: PhantomData,
        })
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ClientMessageType> {
        self.event_tx.subscribe()
    }

    pub fn subscribe_requests(&self) -> broadcast::Receiver<IDedMessage<RequestType>> {
        self.request_tx.subscribe()
    }

    pub async fn send_response(&self, response: IDedMessage<ResponseType>) -> Result<(), Error> {
        let response_msg =
            WrappedServerMessageType::<ResponseType, ServerMessageType>::Response(response);

        // Wait until there is at least one subscriber
        let mut backoff = get_quick_backoff();
        while self.response_tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            backoff.wait().await;
        }

        self.response_tx.send(response_msg)?;
        Ok(())
    }

    pub async fn send_message(&self, message: ServerMessageType) -> Result<(), Error> {
        let message_msg =
            WrappedServerMessageType::<ResponseType, ServerMessageType>::ServerMessage(message);

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

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType> Drop
    for TypedUnixSocketServer<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType:
        Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
{
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.socket_path) {
            tracing::warn!("Failed to remove socket file: {}", e);
        }
    }
}

async fn handle_connection<ClientMessageType, ServerMessageType, RequestType, ResponseType>(
    stream: UnixStream,
    event_tx: broadcast::Sender<ClientMessageType>,
    request_tx: broadcast::Sender<IDedMessage<RequestType>>,
    mut response_rx: broadcast::Receiver<WrappedServerMessageType<ResponseType, ServerMessageType>>,
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
        async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let msg: WrappedClientMessageType<RequestType, ClientMessageType> =
                            match serde_json::from_str(&line) {
                                Ok(msg) => msg,
                                Err(e) => {
                                    tracing::error!("Error deserializing message: {}", e);
                                    continue;
                                }
                            };
                        match msg {
                            WrappedClientMessageType::ClientMessage(event) => {
                                if let Err(e) = event_tx.send(event) {
                                    tracing::error!("Error sending event: {}", e);
                                }
                            }
                            WrappedClientMessageType::Request(request) => {
                                if let Err(e) = request_tx.send(request) {
                                    tracing::error!("Error sending request: {}", e);
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
