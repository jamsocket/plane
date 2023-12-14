use crate::{
    heartbeat_consts::HEARTBEAT_INTERVAL, protocol::MessageFromDrone,
    typed_socket::client::TypedSocketSender, types::NodeStatus,
};
use tokio::task::JoinHandle;

/// A background task that sends heartbeats to the server.
pub struct HeartbeatLoop {
    handle: JoinHandle<()>,
}

impl HeartbeatLoop {
    pub fn start(sender: TypedSocketSender<MessageFromDrone>) -> Self {
        let handle = tokio::spawn(async move {
            loop {
                if let Err(err) = sender.send(MessageFromDrone::Heartbeat {
                    status: NodeStatus::Available,
                }) {
                    tracing::error!(?err, "failed to send heartbeat");
                }

                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            }
        });

        Self { handle }
    }
}

impl Drop for HeartbeatLoop {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
