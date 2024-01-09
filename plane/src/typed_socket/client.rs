use super::{ChannelMessage, Handshake, SocketAction, TypedSocket};
use crate::client::controller_address::AuthorizedAddress;
use crate::names::NodeName;
use crate::{plane_version_info, util::ExponentialBackoff};
use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use std::marker::PhantomData;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tungstenite::handshake::client::generate_key;
use tungstenite::{error::ProtocolError, Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct TypedSocketConnector<T: ChannelMessage> {
    authorized_address: AuthorizedAddress,
    backoff: ExponentialBackoff,
    _phantom: PhantomData<T>,
}

impl<T: ChannelMessage> TypedSocketConnector<T> {
    pub fn new(authorized_address: AuthorizedAddress) -> Self {
        Self {
            authorized_address,
            backoff: ExponentialBackoff::default(),
            _phantom: PhantomData,
        }
    }

    /// Continually retry a connection, with exponential backoff and unlimited
    /// retries.
    ///
    /// This is useful in a connection loop in places that are expected to
    /// always be connected (e.g. the drone).
    pub async fn connect_with_retry(&mut self, name: &impl NodeName) -> TypedSocket<T> {
        loop {
            self.backoff.wait().await;
            match self.connect(name).await {
                Ok(pair) => {
                    self.backoff.defer_reset();
                    return pair;
                }
                Err(e) => {
                    tracing::error!(%e, "Error connecting to server; retrying.");
                }
            }
        }
    }

    pub async fn connect<N: NodeName>(&self, name: &N) -> Result<TypedSocket<T>> {
        let handshake = Handshake {
            name: name.to_string(),
            version: plane_version_info(),
        };

        let req = auth_url_to_request(&self.authorized_address)?;
        let (mut socket, _) = tokio_tungstenite::connect_async(req).await?;

        socket
            .send(Message::Text(serde_json::to_string(&handshake)?))
            .await?;

        let msg = socket
            .next()
            .await
            .ok_or_else(|| anyhow!("Socket closed before handshake received."))??;
        let msg = match msg {
            Message::Text(msg) => msg,
            msg => {
                tracing::error!("Unexpected handshake message: {:?}", msg);
                return Err(anyhow::anyhow!("Handshake message was not text."));
            }
        };

        let remote_handshake: Handshake = serde_json::from_str(&msg)?;
        tracing::info!(
            remote_version = %remote_handshake.version.version,
            remote_hash = %remote_handshake.version.git_hash,
            remote_name = %remote_handshake.name,
            "Connected to server"
        );

        handshake.check_compat(&remote_handshake);

        new_client(socket, remote_handshake).await
    }
}

/// Creates a WebSocket request from an AuthorizedAddress.
fn auth_url_to_request(addr: &AuthorizedAddress) -> Result<hyper::Request<()>> {
    let mut request = hyper::Request::builder()
        .method(hyper::Method::GET)
        .uri(addr.url.as_str())
        .header(
            "Host",
            addr.url
                .host_str()
                .ok_or_else(|| anyhow!("No host in URL."))?
                .to_string(),
        )
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key());

    if let Some(bearer_header) = addr.bearer_header() {
        request = request.header(
            hyper::header::AUTHORIZATION,
            hyper::header::HeaderValue::from_str(&bearer_header)?,
        );
    }

    Ok(request.body(())?)
}

async fn new_client<T: ChannelMessage>(
    mut socket: Socket,
    remote_handshake: Handshake,
) -> Result<TypedSocket<T>> {
    let (send_to_client, recv_to_client) = tokio::sync::mpsc::channel::<T::Reply>(100);
    let (send_from_client, mut recv_from_client) =
        tokio::sync::mpsc::channel::<SocketAction<T>>(100);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                message = recv_from_client.recv() => {
                    match message {
                        None => {
                            let _ = socket.send(Message::Close(None)).await;
                            break;
                        }
                        Some(SocketAction::Send(message)) => {
                            let message = serde_json::to_string(&message).expect("Message is always serializable");
                            if let Err(err) = socket.send(Message::Text(message)).await {
                                tracing::error!(?err, "Failed to send message on websocket.");
                            }
                        },
                        Some(SocketAction::Close) => {
                            recv_from_client.close();
                        }
                    }
                }
                v = socket.next() => {
                    match v {
                        Some(Ok(Message::Text(msg))) => {
                            let result = match serde_json::from_str(&msg) {
                                Ok(msg) => msg,
                                Err(err) => {
                                    tracing::error!(?err, "Failed to deserialize message.");
                                    continue;
                                }
                            };
                            if let Err(e) = send_to_client.send(result).await {
                                tracing::error!(%e, "Error sending message.");
                            }
                        }
                        Some(Err(tungstenite::Error::Protocol(
                            ProtocolError::ResetWithoutClosingHandshake,
                        ))) => {
                            // This is too common to report (it just means the connection was
                            // lost instead of gracefully closed).
                            break;
                        }
                        Some(msg) => {
                            tracing::warn!("Received ignored message: {:?}", msg);
                        }
                        None => {
                            tracing::error!("Connection closed.");
                            break;
                        }
                    }
                }
            }
        }
    });

    Ok(TypedSocket {
        send: send_from_client,
        recv: recv_to_client,
        remote_handshake,
    })
}

#[cfg(test)]
mod test {
    use crate::client::controller_address::AuthorizedAddress;

    #[test]
    fn test_url_no_token() {
        let url = url::Url::parse("https://foo.bar.com/").unwrap();
        let addr = AuthorizedAddress::from(url);
        let request = super::auth_url_to_request(&addr).unwrap();
        assert!(request.headers().get("Authorization").is_none());
    }

    #[test]
    fn test_url_with_token() {
        let url = url::Url::parse("https://abcdefg@foo.bar.com/").unwrap();
        let addr = AuthorizedAddress::from(url);
        let request = super::auth_url_to_request(&addr).unwrap();
        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .map(|d| d.to_str().unwrap()),
            Some("Bearer abcdefg")
        );
        assert_eq!(
            request.headers().get("Host").map(|d| d.to_str().unwrap()),
            Some("foo.bar.com")
        );
    }
}
