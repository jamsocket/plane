use super::{IDedMessage, WrappedClientMessageType, WrappedServerMessageType};
use anyhow::Error;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader, BufWriter};
use tokio::{
    net::UnixStream,
    sync::{broadcast, mpsc, oneshot},
};
use uuid::Uuid;

pub struct TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static,
    ResponseType: Send + Sync + 'static,
{
    // stream: UnixStream,
    tx: mpsc::UnboundedSender<WrappedClientMessageType<RequestType, ClientMessageType>>,
    // rx: mpsc::UnboundedReceiver<WrappedClientMessageType<RequestType, ClientMessageType>>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseType>>>,
    event_tx: broadcast::Sender<ServerMessageType>,
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType>
    TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    ResponseType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
{
    pub async fn new<P: AsRef<Path>>(socket_path: P) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket_path.as_ref()).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let response_map = Arc::new(DashMap::new());
        let (event_tx, _) = broadcast::channel(100);

        tokio::spawn(handle_connection(
            stream,
            rx,
            Arc::clone(&response_map),
            event_tx.clone(),
        ));

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
        self.tx.send(wrapper)?;
        let response = response_rx.await?;
        Ok(response)
    }

    pub async fn send_message(&self, message: ClientMessageType) -> Result<(), Error> {
        let wrapper = WrappedClientMessageType::ClientMessage(message);
        self.tx.send(wrapper)?;
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerMessageType> {
        self.event_tx.subscribe()
    }
}

async fn handle_connection<ClientMessageType, ServerMessageType, RequestType, ResponseType>(
    stream: UnixStream,
    mut rx: mpsc::UnboundedReceiver<WrappedClientMessageType<RequestType, ClientMessageType>>,
    response_map: Arc<DashMap<Uuid, oneshot::Sender<ResponseType>>>,
    event_tx: broadcast::Sender<ServerMessageType>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    ClientMessageType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    ServerMessageType: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de>,
    RequestType: Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
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
        let response_map = Arc::clone(&response_map);
        async move {
            while let Some(line) = lines.next_line().await? {
                let msg: WrappedServerMessageType<ResponseType, ServerMessageType> =
                    serde_json::from_str(&line)?;
                match msg {
                    WrappedServerMessageType::ServerMessage(event) => {
                        let _ = event_tx.send(event);
                    }
                    WrappedServerMessageType::Response(response) => {
                        let id = response.id;
                        if let Some(tx) = response_map.remove(&id).map(|(_, tx)| tx) {
                            let _ = tx.send(response.message);
                        } else {
                            eprintln!("No sender found for response ID: {:?}", id);
                        }
                    }
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    });

    // Task to handle sending messages
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let msg = serde_json::to_string(&msg)?;
            writer.write_all(msg.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });

    let (recv_result, send_result) = tokio::try_join!(recv_task, send_task)?;

    recv_result?;
    send_result?;
    Ok(())
}
