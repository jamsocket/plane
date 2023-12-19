use std::{sync::Arc, thread::JoinHandle};

use super::{ChannelMessage, FullDuplexChannel, Handshake, ping::PingSender};
use crate::plane_version_info;
use anyhow::{anyhow, Context, Result};
use axum::extract::ws::{Message, WebSocket};

pub struct TypedWebsocketServer<T: ChannelMessage> {
    socket: WebSocket,
    handshake: Handshake,
    ping_sender: Arc<PingSender>,
    ping_loop_handle: JoinHandle<()>,
    _phantom: std::marker::PhantomData<T>,
}

pub async fn ping_loop() {

}

impl<T: ChannelMessage> TypedWebsocketServer<T> {
    pub async fn new(mut ws: WebSocket, name: String) -> Result<Self> {
        let handshake = Handshake {
            version: plane_version_info(),
            name,
        };

        let msg = ws
            .recv()
            .await
            .ok_or_else(|| anyhow!("Socket closed before handshake received."))??;
        let msg = match msg {
            Message::Text(msg) => msg,
            msg => {
                tracing::warn!("Received ignored message: {:?}", msg);
                return Err(anyhow!("Handshake message was not text."));
            }
        };
        let client_handshake: Handshake =
            serde_json::from_str(&msg).context("Parsing handshake from client.")?;
        tracing::info!(
            client_version = %client_handshake.version.version,
            client_hash = %client_handshake.version.git_hash,
            client_name = %client_handshake.name,
            "Client connected"
        );

        ws.send(Message::Text(serde_json::to_string(&handshake)?))
            .await?;

        handshake.check_compat(&client_handshake);

        let ping_sender = Arc::new(PingSender::new());

        Ok(Self {
            socket: ws,
            handshake: client_handshake,
            ping_sender,
            _phantom: std::marker::PhantomData,
        })
    }

    pub fn remote_handshake(&self) -> &Handshake {
        &self.handshake
    }
}

#[async_trait::async_trait]
impl<T: ChannelMessage> FullDuplexChannel<T> for TypedWebsocketServer<T> {
    async fn send(&mut self, message: &T) -> Result<()> {
        let message = serde_json::to_string(&message)?;
        self.socket.send(Message::Text(message)).await?;
        Ok(())
    }

    async fn recv(&mut self) -> Option<T::Reply> {
        while let Some(msg) = self.socket.recv().await {
            match msg {
                Ok(Message::Text(msg)) => {
                    let r: T::Reply = match serde_json::from_str(&msg) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(
                                "Failed to parse message: {:?} as {}",
                                e,
                                std::any::type_name::<T::Reply>()
                            );
                            continue;
                        }
                    };
                    return Some(r);
                }
                Ok(Message::Close(_)) => {
                    tracing::info!("Connection closed.");
                    return None;
                }
                Err(e) if e.to_string().contains("ResetWithoutClosingHandshake") => {
                    // ignore (too common)
                }
                msg => {
                    tracing::warn!("Received ignored message: {:?}", msg);
                }
            }
        }

        tracing::warn!("Connection closed.");

        None
    }
}

impl<T: ChannelMessage> Drop for TypedWebsocketServer<T> {
    fn drop(&mut self) {
        tracing::info!("Dropping TypedWebsocketServer");
    }
}
