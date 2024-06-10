use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod client;
pub mod server;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IDedMessage<T> {
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
