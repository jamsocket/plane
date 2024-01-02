use super::PlaneClientError;
use crate::util::ExponentialBackoff;
use hyper::header::{ACCEPT, CONNECTION};
use reqwest::{Client, Response};
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use tungstenite::http::HeaderValue;
use url::Url;

struct RawSseStream {
    response: Response,
    // Bytes left over from the last chunk.
    buffer: Vec<u8>,
    // Bytes that are part of the current data payload.
    data: Option<Vec<u8>>,
    id: Option<String>,
}

impl RawSseStream {
    fn new(response: Response) -> Self {
        Self {
            response,
            buffer: Vec::new(),
            data: None,
            id: None,
        }
    }

    async fn next(&mut self) -> Option<(Option<String>, Vec<u8>)> {
        loop {
            let chunk = match self.response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => return None,
                Err(err) => {
                    tracing::error!(?err, "Error reading SSE stream.");
                    return None;
                }
            };
            let mut chunk = chunk.as_ref();

            // For as long as there are newlines in the chunk, process it line-by-line.
            while let Some(newline_idx) = chunk.iter().position(|&b| b == b'\n') {
                let current_line = &chunk[..newline_idx];
                chunk = &chunk[newline_idx + 1..];

                // If we have anything in the buffer, swap it for an empty buffer and prepend it to the current line.
                let mut buffer = std::mem::take(&mut self.buffer);
                buffer.extend_from_slice(current_line);

                if let Some(result) = buffer.strip_prefix(b"data:") {
                    match self.data {
                        Some(ref mut data) => {
                            // Per https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events#event_stream_format
                            // > When the EventSource receives multiple consecutive lines that begin with data:,
                            // > it concatenates them, inserting a newline character between each one.
                            data.push(b'\n');
                            data.extend_from_slice(result)
                        }
                        None => self.data = Some(result.to_vec()),
                    }
                } else if let Some(result) = buffer.strip_prefix(b"id:") {
                    let id = match String::from_utf8(result.to_vec()) {
                        Ok(id) => id,
                        Err(err) => {
                            tracing::error!(?err, "Error parsing SSE stream ID.");
                            continue;
                        }
                    };
                    self.id = Some(id);
                } else if buffer.is_empty() && self.data.is_some() {
                    let data = self.data.take().unwrap_or_default();
                    return Some((self.id.take(), data));
                }
            }

            // Add anything left over to the buffer.
            self.buffer.extend_from_slice(chunk);
        }
    }
}

pub struct SseStream<T: DeserializeOwned> {
    url: Url,
    client: Client,
    stream: Option<RawSseStream>,
    backoff: ExponentialBackoff,
    last_id: Option<String>,
    _phantom: PhantomData<T>,
}

impl<T: DeserializeOwned> SseStream<T> {
    fn new(url: Url, client: Client) -> Self {
        Self {
            url,
            client,
            stream: None,
            backoff: ExponentialBackoff::default(),
            last_id: None,
            _phantom: PhantomData,
        }
    }

    async fn ensure_stream(&mut self) -> Result<(), PlaneClientError> {
        if self.stream.is_none() {
            let mut request = self
                .client
                .get(self.url.clone())
                .header(ACCEPT, HeaderValue::from_static("text/event-stream"))
                .header(CONNECTION, HeaderValue::from_static("keep-alive"));

            if let Some(id) = &self.last_id {
                request = request.header("Last-Event-ID", id);
            }

            let response = request.send().await?;

            if response.status() != 200 {
                let status = response.status();
                return Err(PlaneClientError::UnexpectedStatus(status));
            }

            self.stream = Some(RawSseStream::new(response));
            return Ok(());
        }

        Ok(())
    }

    pub async fn next(&mut self) -> Option<T> {
        loop {
            if let Err(err) = self.ensure_stream().await {
                tracing::error!(?err, "Error connecting to SSE stream.");
                self.backoff.wait().await;
                continue;
            }

            // We can unwrap here because we just ensured the stream exists.
            let stream = self.stream.as_mut().expect("Stream is always Some.");
            self.backoff.defer_reset();

            let (id, data) = match stream.next().await {
                Some(data) => data,
                None => {
                    self.stream = None;
                    continue;
                }
            };

            self.last_id = id;

            match serde_json::from_slice(&data) {
                Ok(value) => return Some(value),
                Err(err) => {
                    let typ = std::any::type_name::<T>();
                    tracing::error!(?err, typ, "Failed to parse SSE data as type.");
                    continue;
                }
            }
        }
    }
}

pub async fn sse_request<T: DeserializeOwned>(
    url: Url,
    client: Client,
) -> Result<SseStream<T>, PlaneClientError> {
    let mut stream = SseStream::new(url, client);
    stream.ensure_stream().await?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_stream::stream;
    use axum::{
        extract::State,
        response::sse::{Event, KeepAlive, Sse},
        routing::get,
        Router,
    };
    use futures_util::stream::Stream;
    use hyper::HeaderMap;
    use serde::{Deserialize, Serialize};
    use std::{convert::Infallible, time::Duration};
    use tokio::{sync::broadcast, task::JoinHandle, time::timeout};

    #[derive(Serialize, Deserialize, Debug)]
    struct Count {
        value: u32,
    }

    struct DemoSseServer {
        port: u16,
        handle: Option<JoinHandle<std::result::Result<(), hyper::Error>>>,
        disconnect_sender: broadcast::Sender<()>,
    }

    impl Drop for DemoSseServer {
        fn drop(&mut self) {
            self.handle.take().unwrap().abort();
        }
    }

    async fn handle_sse(
        State(disconnect_sender): State<broadcast::Sender<()>>,
        headers: HeaderMap,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let mut receiver = disconnect_sender.subscribe();

        let mut value = headers
            .get("Last-Event-ID")
            .and_then(|id| {
                id.to_str()
                    .ok()
                    .and_then(|id| id.parse::<u32>().ok())
                    .map(|id| id + 1)
            })
            .unwrap_or(0);

        let stream = stream! {
            loop {
                match timeout(Duration::from_millis(100), receiver.recv()).await {
                    Ok(_) => break,
                    Err(_) => ()
                };

                let event = Event::default().json_data(&Count { value }).unwrap().id(value.to_string());
                yield Ok(event);
                value += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };

        Sse::new(stream).keep_alive(KeepAlive::default())
    }

    impl DemoSseServer {
        fn new() -> Self {
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (disconnect_sender, _) = broadcast::channel::<()>(1);

            let app = Router::new()
                .route("/counter", get(handle_sse))
                .with_state(disconnect_sender.clone());
            let server = axum::Server::from_tcp(listener)
                .unwrap()
                .serve(app.into_make_service());
            let handle = tokio::spawn(server);

            Self {
                port,
                handle: Some(handle),
                disconnect_sender,
            }
        }

        async fn disconnect(&self) {
            self.disconnect_sender.send(()).unwrap();
        }

        fn url(&self) -> Url {
            let url = format!("http://localhost:{}/counter", self.port);
            url::Url::parse(&url).unwrap()
        }
    }

    #[tokio::test]
    async fn test_simple_sse() {
        let server = DemoSseServer::new();

        let client = reqwest::Client::new();
        let mut stream = super::sse_request::<Count>(server.url(), client)
            .await
            .unwrap();

        for i in 0..10 {
            let value = stream.next().await.unwrap();
            assert_eq!(value.value, i);
        }
    }

    #[tokio::test]
    async fn test_sse_reconnect() {
        let server = DemoSseServer::new();

        let client = reqwest::Client::new();
        let mut stream = super::sse_request::<Count>(server.url(), client)
            .await
            .unwrap();

        for i in 0..10 {
            let value = stream.next().await.unwrap();
            assert_eq!(value.value, i);
        }

        server.disconnect().await;

        for i in 10..20 {
            let value = stream.next().await.unwrap();
            assert_eq!(value.value, i);
        }
    }
}
