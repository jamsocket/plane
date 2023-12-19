use crate::PlaneVersionInfo;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use tokio::sync::mpsc::{Receiver, Sender};

pub mod client;
pub mod server;
mod ping;

pub enum SocketAction<T> {
    Send(T),
    Close,
}

pub trait ChannelMessage: Send + Sync + 'static + DeserializeOwned + Serialize + Debug {
    type Reply: ChannelMessage<Reply = Self>;
}

pub struct TypedSocket<T: ChannelMessage> {
    send: Sender<SocketAction<T>>,
    recv: Receiver<T::Reply>,
    pub remote_handshake: Handshake,
}

pub struct TypedSocketSender<T: ChannelMessage> {
    send: Sender<SocketAction<T>>,
}

impl<T: ChannelMessage> TypedSocketSender<T> {
    pub fn send(&self, message: T) -> anyhow::Result<()> {
        self.send.try_send(SocketAction::Send(message))?;
        Ok(())
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.send.send(SocketAction::Close).await?;
        Ok(())
    }
}

impl<T: ChannelMessage> TypedSocket<T> {
    pub async fn send(&mut self, message: T) -> anyhow::Result<()> {
        self.send.send(SocketAction::Send(message)).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<T::Reply> {
        self.recv.recv().await
    }

    pub fn sender(&self) -> TypedSocketSender<T> {
        TypedSocketSender {
            send: self.send.clone(),
        }
    }

    pub async fn close(&mut self) {
        let _ = self.send.send(SocketAction::Close).await;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Handshake {
    pub version: PlaneVersionInfo,
    pub name: String,
}

impl Handshake {
    pub fn check_compat(&self, other: &Handshake) {
        if self.version.version != other.version.version {
            tracing::warn!(
                "Client and server have different Plane versions: {} (local) != {} (remote).",
                self.version.version,
                other.version.version
            );
        } else if self.version.git_hash != other.version.git_hash {
            tracing::warn!(
                "Client and server have different Plane git hashes: {} (local) != {} (remote).",
                self.version.git_hash,
                other.version.git_hash
            );
        }
    }
}
