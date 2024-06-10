use super::{IDedMessage, WrappedClientMessageType, WrappedServerMessageType};
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
    sync::{broadcast, mpsc},
};

pub struct TypedUnixSocketServer<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static + Clone,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static + Clone + Debug,
    ResponseType: Send + Sync + 'static,
{
    socket_path: PathBuf,
    event_tx: broadcast::Sender<ClientMessageType>,
    request_tx: broadcast::Sender<IDedMessage<RequestType>>,
    response_tx: mpsc::UnboundedSender<WrappedServerMessageType<ResponseType, ServerMessageType>>,
    _phantom: PhantomData<(ServerMessageType, ResponseType)>,
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType>
    TypedUnixSocketServer<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
{
    pub async fn new<P: AsRef<Path>>(socket_path: P) -> Result<Self, Error> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let listener = UnixListener::bind(&socket_path)?;
        let (event_tx, _) = broadcast::channel(100);
        let (request_tx, _) = broadcast::channel(100);
        let (response_tx, response_rx) = mpsc::unbounded_channel();

        tokio::spawn({
            let event_tx = event_tx.clone();
            let request_tx = request_tx.clone();
            async move {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        handle_connection(stream, event_tx, request_tx, response_rx)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::warn!("Error handling connection: {}", e);
                            });
                    }
                    Err(e) => {
                        tracing::warn!("Error accepting connection: {}", e);
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
        self.response_tx.send(response_msg)?;
        Ok(())
    }

    pub async fn send_message(&self, message: ServerMessageType) -> Result<(), Error> {
        let message_msg =
            WrappedServerMessageType::<ResponseType, ServerMessageType>::ServerMessage(message);
        self.response_tx.send(message_msg)?;
        Ok(())
    }
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType> Drop
    for TypedUnixSocketServer<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static + Clone,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static + Clone + Debug,
    ResponseType: Send + Sync + 'static,
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
    mut response_rx: mpsc::UnboundedReceiver<
        WrappedServerMessageType<ResponseType, ServerMessageType>,
    >,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    ClientMessageType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Clone + Debug + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
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
            while let Some(line) = lines.next_line().await? {
                let msg: WrappedClientMessageType<RequestType, ClientMessageType> =
                    serde_json::from_str(&line)?;
                match msg {
                    WrappedClientMessageType::ClientMessage(event) => {
                        let _ = event_tx.send(event);
                    }
                    WrappedClientMessageType::Request(request) => {
                        request_tx.send(request)?;
                    }
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    });

    // Task to handle sending responses
    let send_task = tokio::spawn(async move {
        while let Some(response) = response_rx.recv().await {
            let response_str = serde_json::to_string(&response)?;
            writer.write_all(response_str.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });

    tokio::try_join!(recv_task, send_task)?;
    Ok(())
}
