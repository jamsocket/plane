use crate::PlaneVersionInfo;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError;
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

#[derive(Clone)]
pub struct TypedSocketSender<A> {
    inner_send:
        Arc<dyn Fn(SocketAction<A>) -> Result<(), TypedSocketError> + 'static + Send + Sync>,
}

impl<T> Debug for TypedSocketSender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("typed socket sender")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TypedSocketError {
    #[error("Socket closed")]
    Closed,
    #[error("Socket disconnected")]
    Disconnected,
}

impl<A> From<TrySendError<A>> for TypedSocketError {
    fn from(e: TrySendError<A>) -> Self {
        match e {
            TrySendError::Full(_) => Self::Disconnected,
            TrySendError::Closed(_) => Self::Closed,
        }
    }
}

impl<A: Debug> TypedSocketSender<A> {
    #[tracing::instrument]
    pub fn send(&self, message: A) -> Result<(), TypedSocketError> {
        (self.inner_send)(SocketAction::Send(message))?;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), TypedSocketError> {
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
            sender.try_send(message).map_err(|e| e.into())
        };

        TypedSocketSender {
            inner_send: Arc::new(inner_send),
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
    /// Compare a local and remote handshake, and log a warning if they are not compatible.
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
