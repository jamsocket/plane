use crate::util::ExponentialBackoff;
use chrono::Duration;
use serde::{Deserialize, Serialize};

pub mod client;
pub mod server;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WrappedMessage<T> {
    /// Optional ID for this message. If it is provided, this message belongs to a request/response pair
    /// (either as the request or the response). If it is not provided, this message is an event.
    id: Option<String>,
    message: T,
}

fn get_quick_backoff() -> ExponentialBackoff {
    ExponentialBackoff::new(
        Duration::try_milliseconds(10).expect("duration is always valid"),
        Duration::try_milliseconds(100).expect("duration is always valid"),
        1.1,
        Duration::try_milliseconds(100).expect("duration is always valid"),
    )
}

#[cfg(test)]
mod tests {
    use super::client::TypedUnixSocketClient;
    use super::server::TypedUnixSocketServer;
    use crate::util::random_string;
    use futures_util::future::join_all;
    use std::collections::HashSet;
    use std::env;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::spawn;

    fn create_temp_socket_path() -> PathBuf {
        // We generate a random string to use as the socket path.
        // Due to limitations of the underlying syscall (ref: https://man7.org/linux/man-pages/man7/unix.7.html),
        // the total length of the socket path is limited to 108 characters.
        // We use random_string() rather than random_token() to minimize the chance of exceeding this limit.
        let path = env::temp_dir().join(format!("test_socket_{}", random_string()));
        if path.to_str().unwrap().len() > 107 {
            panic!(
                "The socket path ({:?}) is too long. The maximum length is 108 characters (including terminating null). Try running the tests in a shallower directory.",
                path
            );
        }
        path
    }

    #[tokio::test]
    async fn test_request_response() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        // Spawn a task to handle server requests
        spawn(async move {
            let mut request_rx = server.subscribe_requests();
            while let Ok(request) = request_rx.recv().await {
                let response = "Hello, client!".to_string();
                server.send_response(&request, response).await.unwrap();
            }
        });

        let request = "Hello, server!".to_string();
        let response = client.send_request(request).await.unwrap();

        assert_eq!(response, "Hello, client!");
    }

    #[tokio::test]
    async fn test_client_to_server_ad_hoc() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(socket_path.to_str().unwrap())
            .await
            .unwrap();
        let client =
            TypedUnixSocketClient::<String, String>::new(socket_path.to_str().unwrap()).await;

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

        let server = TypedUnixSocketServer::<String, String>::new(socket_path.to_str().unwrap())
            .await
            .unwrap();
        let client =
            TypedUnixSocketClient::<String, String>::new(socket_path.to_str().unwrap()).await;

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
    async fn test_concurrent_requests_responses() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        // Spawn a task to handle server requests
        spawn(async move {
            let mut request_rx = server.subscribe_requests();
            while let Ok(request) = request_rx.recv().await {
                let response = format!("Response to {}", request.message);
                server.send_response(&request, response).await.unwrap();
            }
        });

        let mut handles = vec![];
        let client = Arc::new(client);
        for i in 0..10 {
            let client = client.clone();
            handles.push(spawn(async move {
                let request = format!("Request {}", i);
                let response = client.send_request(request).await.unwrap();
                assert_eq!(response, format!("Response to Request {}", i));
            }));
        }

        join_all(handles).await;
    }

    #[tokio::test]
    async fn test_concurrent_client_to_server_messages() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        // Subscribe to server events
        let mut event_rx = server.subscribe_events();

        let mut handles = vec![];
        let client = Arc::new(client);
        for i in 0..10 {
            let client = client.clone();
            handles.push(spawn(async move {
                let message = format!("Message {}", i);
                client.send_message(message.clone()).await.unwrap();
            }));
        }

        join_all(handles).await;

        let mut received_messages = HashSet::new();
        for _ in 0..10 {
            let received_message = event_rx.recv().await.unwrap();
            received_messages.insert(received_message);
        }

        for i in 0..10 {
            assert!(received_messages.contains(&format!("Message {}", i)));
        }
    }

    #[tokio::test]
    async fn test_concurrent_server_to_client_messages() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        // Subscribe to client events
        let mut event_rx = client.subscribe_events();

        let mut handles = vec![];
        let server = Arc::new(server);
        for i in 0..10 {
            let server = server.clone();
            handles.push(spawn(async move {
                let message = format!("Message {}", i);
                server.send_message(message.clone()).await.unwrap();
            }));
        }

        join_all(handles).await;

        let mut received_messages = HashSet::new();
        for _ in 0..10 {
            let received_message = event_rx.recv().await.unwrap();
            received_messages.insert(received_message);
        }

        for i in 0..10 {
            assert!(received_messages.contains(&format!("Message {}", i)));
        }
    }

    #[tokio::test]
    async fn test_client_restart() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        // Spawn a task to handle server requests
        let server = server.clone();
        spawn(async move {
            let mut request_rx = server.subscribe_requests();
            while let Ok(request) = request_rx.recv().await {
                let response = "Hello, client!".to_string();
                server.send_response(&request, response).await.unwrap();
            }
        });

        // Simulate client restart
        drop(client);
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        let request = "Hello, server!".to_string();
        let response = client.send_request(request).await.unwrap();
        assert_eq!(response, "Hello, client!");
    }

    #[tokio::test]
    async fn test_server_restart() {
        let socket_path = create_temp_socket_path();

        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();
        let client = TypedUnixSocketClient::<String, String>::new(&socket_path).await;

        // Simulate server restart
        drop(server);
        let server = TypedUnixSocketServer::<String, String>::new(&socket_path)
            .await
            .unwrap();

        // Spawn a task to handle server requests
        let server = server.clone();
        spawn(async move {
            let mut request_rx = server.subscribe_requests();
            while let Ok(request) = request_rx.recv().await {
                let response = "Hello, client!".to_string();
                server.send_response(&request, response).await.unwrap();
            }
        });

        let request = "Hello, server!".to_string();
        let response = client.send_request(request).await.unwrap();
        assert_eq!(response, "Hello, client!");
    }
}
