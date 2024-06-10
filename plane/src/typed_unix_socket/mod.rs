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
    use std::env;
    use std::path::PathBuf;
    use tokio::spawn;
    use uuid::Uuid;

    fn create_temp_socket_path() -> PathBuf {
        env::temp_dir().join(format!("test_socket_{}", Uuid::new_v4())) // ensure random file name
    }

    #[tokio::test]
    async fn test_request_response() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String, String, String>::new(&socket_path)
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

    #[tokio::test]
    async fn test_client_to_server_ad_hoc() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String, String, String>::new(
            socket_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let client = TypedUnixSocketClient::<String, String, String, String>::new(
            socket_path.to_str().unwrap(),
        )
        .await
        .unwrap();

        // Subscribe to server events
        let mut event_rx = server.subscribe_events();

        // Send an ad-hoc message from the client
        let ad_hoc_message = "Ad-hoc message from client".to_string();
        client.send_message(ad_hoc_message.clone()).await.unwrap();

        // Receive the ad-hoc message on the server
        let received_message = event_rx.recv().await.unwrap();
        assert_eq!(received_message, ad_hoc_message);
    }

    #[tokio::test]
    async fn test_server_to_client_ad_hoc() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String, String, String>::new(
            socket_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let client = TypedUnixSocketClient::<String, String, String, String>::new(
            socket_path.to_str().unwrap(),
        )
        .await
        .unwrap();

        // Subscribe to client events
        let mut event_rx = client.subscribe_events();

        // Send an ad-hoc message from the server
        let ad_hoc_message = "Ad-hoc message from server".to_string();
        server.send_message(ad_hoc_message.clone()).await.unwrap();

        // Receive the ad-hoc message on the client
        let received_message = event_rx.recv().await.unwrap();
        assert_eq!(received_message, ad_hoc_message);
    }

    #[tokio::test]
    async fn test_multiple_concurrent_requests_responses() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();

        // Spawn a task to handle server requests
        spawn(async move {
            let mut request_rx = server.subscribe_requests();
            while let Ok(request) = request_rx.recv().await {
                let response = IDedMessage {
                    id: request.id,
                    message: format!("Response to {}", request.message),
                };
                server.send_response(response).await.unwrap();
            }
        });

        let mut handles = vec![];
        for i in 0..10 {
            let client = client.clone();
            handles.push(spawn(async move {
                let request = format!("Request {}", i);
                let response = client.send_request(request).await.unwrap();
                assert_eq!(response, format!("Response to Request {}", i));
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_concurrent_client_to_server_messages() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();

        // Subscribe to server events
        let mut event_rx = server.subscribe_events();

        let mut handles = vec![];
        for i in 0..10 {
            let client = client.clone();
            handles.push(spawn(async move {
                let message = format!("Message {}", i);
                client.send_message(message.clone()).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        for i in 0..10 {
            let received_message = event_rx.recv().await.unwrap();
            assert_eq!(received_message, format!("Message {}", i));
        }
    }

    #[tokio::test]
    async fn test_concurrent_server_to_client_messages() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String, String, String>::new(&socket_path)
            .await
            .unwrap();

        // Subscribe to client events
        let mut event_rx = client.subscribe_events();

        let mut handles = vec![];
        for i in 0..10 {
            let server = server.clone();
            handles.push(spawn(async move {
                let message = format!("Message {}", i);
                server.send_message(message.clone()).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        for i in 0..10 {
            let received_message = event_rx.recv().await.unwrap();
            assert_eq!(received_message, format!("Message {}", i));
        }
    }
}
