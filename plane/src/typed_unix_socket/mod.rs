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

#[cfg(test)]
mod tests {
    use super::client::TypedUnixSocketClient;
    use super::server::TypedUnixSocketServer;
    use super::IDedMessage;
    use tokio::spawn;

    #[tokio::test]
    async fn test_unix_socket() {
        let server =
            TypedUnixSocketServer::<String, String, String, String>::new("/tmp/test_socket")
                .await
                .unwrap();
        let client =
            TypedUnixSocketClient::<String, String, String, String>::new("/tmp/test_socket")
                .await
                .unwrap();

        // Spawn a task to handle server requests
        spawn(async move {
            let mut request_rx = server.subscribe_requests();
            while let Ok(request) = request_rx.recv().await {
                let response = IDedMessage {
                    id: request.id,
                    message: "Hello, client!".to_string(),
                };
                server.send_response(response).await.unwrap();
            }
        });

        let request = "Hello, server!".to_string();
        let response = client.send_request(request).await.unwrap();

        assert_eq!(response, "Hello, client!");
    }
}
