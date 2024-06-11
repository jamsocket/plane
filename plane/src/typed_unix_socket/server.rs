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
    signal,
    sync::broadcast,
    sync::watch,
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
    shutdown_tx: watch::Sender<()>,
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
        let (shutdown_tx, shutdown_rx) = watch::channel(());

        // Handle ctrl_c signal
        tokio::spawn({
            let shutdown_tx = shutdown_tx.clone();
            async move {
                signal::ctrl_c()
                    .await
                    .expect("Failed to install Ctrl+C handler");
                tracing::info!("Ctrl+C received, initiating shutdown...");
                let _ = shutdown_tx.send(());
            }
        });

        // Accept connections and handle them
        tokio::spawn({
            let event_tx = event_tx.clone();
            let request_tx = request_tx.clone();
            let response_tx = response_tx.clone();
            let mut shutdown_rx = shutdown_rx.clone();
            async move {
                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            tracing::info!("Shutdown signal received.");
                            break;
                        }
                        result = listener.accept() => match result {
                            Ok((stream, _)) => {
                                tokio::spawn(handle_connection(
                                    stream,
                                    event_tx.clone(),
                                    request_tx.clone(),
                                    response_tx.subscribe(),
                                    shutdown_rx.clone(),
                                ));
                            }
                            Err(e) => {
                                tracing::error!("Error accepting connection: {}", e);
                            }
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
            shutdown_tx,
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
        while self.response_tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        self.response_tx.send(response_msg)?;
        Ok(())
    }

    pub async fn send_message(&self, message: ServerMessageType) -> Result<(), Error> {
        let message_msg =
            WrappedServerMessageType::<ResponseType, ServerMessageType>::ServerMessage(message);

        // Wait until there is at least one subscriber
        while self.response_tx.receiver_count() == 0 {
            tracing::info!("Waiting for a subscriber...");
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
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
        if let Err(e) = self.shutdown_tx.send(()) {
            tracing::warn!("Failed to send shutdown signal: {}", e);
        }
    }
}

async fn handle_connection<ClientMessageType, ServerMessageType, RequestType, ResponseType>(
    stream: UnixStream,
    event_tx: broadcast::Sender<ClientMessageType>,
    request_tx: broadcast::Sender<IDedMessage<RequestType>>,
    mut response_rx: broadcast::Receiver<WrappedServerMessageType<ResponseType, ServerMessageType>>,
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
                                match serde_json::from_str::<WrappedClientMessageType<RequestType, ClientMessageType>>(&line) {
                                    Ok(msg) => {
                                        match msg {
                                            WrappedClientMessageType::ClientMessage(event) => {
                                                if let Err(e) = event_tx.send(event) {
                                                    tracing::error!("Failed to send client message: {:?}", e);
                                                }
                                            }
                                            WrappedClientMessageType::Request(request) => {
                                                if let Err(e) = request_tx.send(request) {
                                                    tracing::error!("Failed to send request: {:?}", e);
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

    // Task to handle sending responses
    let send_task = tokio::spawn({
        let mut shutdown_rx = shutdown_rx.clone();
        async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Shutdown signal received in send_task.");
                        break;
                    }
                    response = response_rx.recv() => {
                        match response {
                            Ok(response) => {
                                match serde_json::to_string(&response) {
                                    Ok(response_str) => {
                                        if let Err(e) = writer.write_all(response_str.as_bytes()).await {
                                            tracing::error!("Failed to write response: {:?}", e);
                                        }
                                        if let Err(e) = writer.write_all(b"\n").await {
                                            tracing::error!("Failed to write newline: {:?}", e);
                                        }
                                        if let Err(e) = writer.flush().await {
                                            tracing::error!("Failed to flush writer: {:?}", e);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to serialize response: {:?}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to receive response: {:?}", e);
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
