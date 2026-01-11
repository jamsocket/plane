use crate::version::PlaneVersionInfo;
use crate::PlaneClientError;
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
pub struct TypedSocketSender<T: ChannelMessage> {
    sender: Sender<SocketAction<T>>,
}

#[derive(Clone)]
pub struct WrappedTypedSocketSender<K> {
    send: Arc<dyn Fn(K) -> Result<(), TypedSocketError> + 'static + Send + Sync>,
}

impl<K> WrappedTypedSocketSender<K> {
    pub fn new<T: ChannelMessage, F>(sender: Sender<SocketAction<T>>, transform: F) -> Self
    where
        F: (Fn(K) -> T) + 'static + Send + Sync,
    {
        Self {
            send: Arc::new(move |message| {
                sender
                    .try_send(SocketAction::Send(transform(message)))
                    .map_err(TypedSocketError::from)
            }),
        }
    }

    pub fn send(&self, message: K) -> Result<(), TypedSocketError> {
        (self.send)(message)
    }
}

impl<T: ChannelMessage> Debug for TypedSocketSender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("typed socket sender")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TypedSocketError {
    #[error("Receiver closed")]
    Closed,
    #[error("Receiver queue full")]
    Clogged,
}

impl<A> From<TrySendError<A>> for TypedSocketError {
    fn from(e: TrySendError<A>) -> Self {
        match e {
            TrySendError::Full(_) => Self::Clogged,
            TrySendError::Closed(_) => Self::Closed,
        }
    }
}

impl<T: ChannelMessage> TypedSocketSender<T> {
    pub fn send(&self, message: T) -> Result<(), TypedSocketError> {
        self.sender.try_send(SocketAction::Send(message))?;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), TypedSocketError> {
        self.sender.try_send(SocketAction::Close)?;
        Ok(())
    }

    /// Wrap the sender with a transform function.
    pub fn wrap<K, F>(&self, transform: F) -> WrappedTypedSocketSender<K>
    where
        F: (Fn(K) -> T) + 'static + Send + Sync,
    {
        WrappedTypedSocketSender::new(self.sender.clone(), transform)
    }
}

impl<T: ChannelMessage> TypedSocket<T> {
    pub fn send(&mut self, message: T) -> Result<(), PlaneClientError> {
        self.send
            .try_send(SocketAction::Send(message))
            .map_err(|_| PlaneClientError::SendFailed)?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<T::Reply> {
        self.recv.recv().await
    }

    pub fn sender(&self) -> TypedSocketSender<T> {
        let sender = self.send.clone();

        TypedSocketSender { sender }
    }

    pub async fn close(&mut self) {
        let _ = self.send.send(SocketAction::Close).await;
    }

    /// Returns (pending_outgoing, pending_incoming) message counts
    pub fn channel_depths(&self) -> (usize, usize) {
        let outgoing = self.send.max_capacity() - self.send.capacity();
        let incoming = self.recv.len();
        (outgoing, incoming)
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
                local_version = self.version.version,
                remote_version = other.version.version,
                "Client and server have different Plane versions."
            );
        } else if self.version.git_hash != other.version.git_hash {
            tracing::warn!(
                local_version = self.version.git_hash,
                remote_version = other.version.git_hash,
                "Client and server have different Plane git hashes.",
            );
        }
    }
}
