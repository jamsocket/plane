use crate::PlaneVersionInfo;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use tokio::sync::mpsc::{Receiver, Sender};

pub mod client;
pub mod server;

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

pub struct TypedSocketSender<A> {
    inner_send: Box<dyn Fn(SocketAction<A>) -> anyhow::Result<()> + 'static + Send + Sync>,
}

impl<A> TypedSocketSender<A> {
    pub fn send(&self, message: A) -> anyhow::Result<()> {
        (self.inner_send)(SocketAction::Send(message))?;
        Ok(())
    }

    pub fn close(&mut self) -> anyhow::Result<()> {
        (self.inner_send)(SocketAction::Close)?;
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

    pub fn sender<A, F>(&self, transform: F) -> TypedSocketSender<A>
    where
        F: (Fn(A) -> T) + 'static + Send + Sync,
    {
        let sender = self.send.clone();
        let inner_send = move |message: SocketAction<A>| {
            let message = match message {
                SocketAction::Close => SocketAction::Close,
                SocketAction::Send(message) => SocketAction::Send(transform(message)),
            };
            sender.try_send(message).map_err(|e| anyhow::anyhow!(e))
        };

        TypedSocketSender {
            inner_send: Box::new(inner_send),
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
