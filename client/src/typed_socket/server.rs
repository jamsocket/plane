use super::{ChannelMessage, Handshake, SocketAction, TypedSocket};
use crate::plane_version_info;
use anyhow::{anyhow, Context, Result};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use tokio::sync::mpsc::{Receiver, Sender};

pub async fn handle_messages<T: ChannelMessage>(
    mut messages_to_send: Receiver<SocketAction<T>>,
    messages_received: Sender<T::Reply>,
    mut socket: WebSocket,
) {
    loop {
        tokio::select! {
            Some(msg) = messages_to_send.recv() => {
                match msg {
                    SocketAction::Send(msg) => {
                        let msg = Message::Text(serde_json::to_string(&msg).expect("Always serializable."));
                        if let Err(err) = socket.send(msg.clone()).await {
                            tracing::error!(?err, message=?msg, "Failed to send message on websocket.");
                        }
                    }
                    SocketAction::Close => {
                        if let Err(err) = socket.close().await {
                            tracing::error!(?err, "Failed to close websocket.");
                        }
                        break;
                    }
                }
            }
            Some(msg) = socket.recv() => {
                let msg = match msg {
                    Ok(Message::Text(msg)) => msg,
                    Err(err) => {
                        tracing::error!(?err, "Failed to receive message from websocket.");
                        break;
                    }
                    Ok(Message::Close(Some(CloseFrame { code: 1001, .. }))) => {
                        tracing::warn!("Websocket connection closed.");
                        break;
                    }
                    msg => {
                        tracing::warn!("Received ignored message: {:?}", msg);
                        continue;
                    }
                };
                let msg: T::Reply = match serde_json::from_str(&msg) {
                    Ok(msg) => msg,
                    Err(err) => {
                        tracing::warn!(?err, "Failed to parse message.");
                        continue;
                    }
                };
                if let Err(err) = messages_received.send(msg).await {
                    tracing::error!(?err, "Failed to receive message.");
                    break;
                }
            }
            else => {
                break;
            }
        }
    }
}

pub async fn new_server<T: ChannelMessage>(
    mut ws: WebSocket,
    name: String,
) -> Result<TypedSocket<T>> {
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
    let remote_handshake: Handshake =
        serde_json::from_str(&msg).context("Parsing handshake from client.")?;
    tracing::info!(
        client_version = %remote_handshake.version.version,
        client_hash = %remote_handshake.version.git_hash,
        client_name = %remote_handshake.name,
        "Client connected"
    );

    let local_handshake = Handshake {
        version: plane_version_info(),
        name,
    };
    ws.send(Message::Text(serde_json::to_string(&local_handshake)?))
        .await?;

    local_handshake.check_compat(&remote_handshake);

    let (outgoing_message_sender, outgoing_message_receiver) = tokio::sync::mpsc::channel(100);
    let (incoming_message_sender, incoming_message_receiver) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move {
        handle_messages(outgoing_message_receiver, incoming_message_sender, ws).await;
    });

    Ok(TypedSocket {
        send: outgoing_message_sender,
        recv: incoming_message_receiver,
        remote_handshake,
    })
}
