use anyhow::Error;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::UnixStream,
    sync::{broadcast, mpsc, oneshot},
};
use tokio_serde::{formats::Json, Framed as SerdeFramed};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct IDedMessage<T> {
    id: Uuid,
    message: T,
}

enum WrappedServerMessageType<ResponseType, ServerMessageType> {
    Response(IDedMessage<ResponseType>),
    ServerMessage(ServerMessageType),
}

enum WrappedClientMessageType<RequestType, ClientMessageType> {
    Request(IDedMessage<RequestType>),
    ClientMessage(ClientMessageType),
}

struct TypedUnixSocket<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static,
    ResponseType: Send + Sync + 'static,
{
    socket_path: PathBuf,
}

struct TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
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
    TypedUnixSocket<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static,
    ResponseType: Send + Sync + 'static,
{
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub async fn create_client(
        &self,
    ) -> TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
    {
        TypedUnixSocketClient::new(&self.socket_path).await.unwrap()
    }

    // pub async fn create_server(&self) -> TypedUnixSocketServer<ClientMessageType, ServerMessageType> {
    //     //TODO
    //     unimplemented!()
    // }
}

impl<ClientMessageType, ServerMessageType, RequestType, ResponseType>
    TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
where
    ClientMessageType: Send + Sync + 'static,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static,
    ResponseType: Send + Sync + 'static,
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
    ClientMessageType: Send + Sync + 'static,
    ServerMessageType: Send + Sync + 'static + Clone,
    RequestType: Send + Sync + 'static,
    ResponseType: Send + Sync + 'static,
{
    let length_delimited = Framed::new(stream, LengthDelimitedCodec::new());
    let mut framed = SerdeFramed::new(
        length_delimited,
        Json::<
            WrappedClientMessageType<RequestType, ClientMessageType>,
            WrappedServerMessageType<ResponseType, ServerMessageType>,
        >::default(),
    );

    // Task to handle receiving messages
    let recv_task = tokio::spawn({
        let event_tx = event_tx.clone();
        let response_map = Arc::clone(&response_map);
        async move {
            while let Some(Ok(msg)) = framed.try_next().await {
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
