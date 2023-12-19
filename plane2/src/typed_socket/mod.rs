use crate::PlaneVersionInfo;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub mod client;
pub mod server;
mod ping;

pub trait ChannelMessage: Send + Sync + 'static + DeserializeOwned + Serialize + Debug {
    type Reply: ChannelMessage<Reply = Self>;
}

#[async_trait::async_trait]
pub trait FullDuplexChannel<T: ChannelMessage> {
    async fn send(&mut self, message: &T) -> anyhow::Result<()>;

    async fn recv(&mut self) -> Option<T::Reply>;
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
