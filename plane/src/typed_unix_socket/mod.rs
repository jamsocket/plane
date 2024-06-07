use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

pub mod client;

#[derive(Serialize, Deserialize)]
struct IDedMessage<T> {
    id: Uuid,
    message: T,
}

#[derive(Serialize, Deserialize)]
enum WrappedServerMessageType<ResponseType, ServerMessageType> {
    Response(IDedMessage<ResponseType>),
    ServerMessage(ServerMessageType),
}

#[derive(Serialize, Deserialize)]
enum WrappedClientMessageType<RequestType, ClientMessageType> {
    Request(IDedMessage<RequestType>),
    ClientMessage(ClientMessageType),
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

// struct TypedUnixSocket<ClientMessageType, ServerMessageType, RequestType, ResponseType>
// where
//     ClientMessageType: Send + Sync + 'static,
//     ServerMessageType: Send + Sync + 'static + Clone,
//     RequestType: Send + Sync + 'static,
//     ResponseType: Send + Sync + 'static,
// {
//     socket_path: PathBuf,
// }

// impl<ClientMessageType, ServerMessageType, RequestType, ResponseType>
//     TypedUnixSocket<ClientMessageType, ServerMessageType, RequestType, ResponseType>
// where
//     ClientMessageType: Send + Sync + 'static,
//     ServerMessageType: Send + Sync + 'static + Clone,
//     RequestType: Send + Sync + 'static,
//     ResponseType: Send + Sync + 'static,
// {
//     pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
//         Self {
//             socket_path: socket_path.into(),
//         }
//     }

//     pub async fn create_client(
//         &self,
//     ) -> TypedUnixSocketClient<ClientMessageType, ServerMessageType, RequestType, ResponseType>
//     {
//         TypedUnixSocketClient::new(&self.socket_path).await.unwrap()
//     }

//     // pub async fn create_server(&self) -> TypedUnixSocketServer<ClientMessageType, ServerMessageType> {
//     //     //TODO
//     //     unimplemented!()
//     // }
// }
