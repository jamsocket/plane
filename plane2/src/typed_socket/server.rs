use super::{ChannelMessage, FullDuplexChannel, Handshake};
use crate::plane_version_info;
use anyhow::{anyhow, Context, Result};
use axum::extract::ws::{Message, WebSocket};
use tokio::{
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};

pub struct TypedWebsocketServer<T: ChannelMessage> {
    handshake: Handshake,
    handle: JoinHandle<()>,
    outgoing_message_sender: Sender<Message>,
    incoming_message_receiver: Receiver<Message>,
    _phantom: std::marker::PhantomData<T>,
}

pub async fn handle_messages(
    mut messages_to_send: Receiver<Message>,
    messages_received: Sender<Message>,
    mut socket: WebSocket,
) {
    loop {
        tokio::select! {
            Some(msg) = messages_to_send.recv() => {
                socket.send(msg).await.unwrap();
            }
            Some(msg) = socket.recv() => {
                messages_received.send(msg.unwrap()).await.unwrap();
            }
            else => {
                break;
            }
        }
    }
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

        let (outgoing_message_sender, outgoing_message_receiver) = tokio::sync::mpsc::channel(100);
        let (incoming_message_sender, incoming_message_receiver) = tokio::sync::mpsc::channel(100);
        let handle = tokio::spawn(async move {
            handle_messages(outgoing_message_receiver, incoming_message_sender, ws).await;
        });

        Ok(Self {
            handle,
            outgoing_message_sender,
            incoming_message_receiver,
            handshake: client_handshake,
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
        self.outgoing_message_sender
            .send(Message::Text(message))
            .await?;
        Ok(())
    }

    async fn recv(&mut self) -> Option<T::Reply> {
        while let Some(msg) = self.incoming_message_receiver.recv().await {
            match msg {
                Message::Text(msg) => {
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
                Message::Close(_) => {
                    tracing::info!("Connection closed.");
                    return None;
                }
                msg => {
                    tracing::warn!("Received ignored message: {:?}", msg);
                }
            }
        }

        tracing::warn!("Connection closed11.");

        None
    }
}

impl<T: ChannelMessage> Drop for TypedWebsocketServer<T> {
    fn drop(&mut self) {
        self.handle.abort();
        tracing::info!("Dropping TypedWebsocketServer");
    }
}
