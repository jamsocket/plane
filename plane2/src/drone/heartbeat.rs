use crate::{
    heartbeat_consts::HEARTBEAT_INTERVAL, protocol::MessageFromDrone,
    typed_socket::client::TypedSocketSender,
};
use std::time::SystemTime;
use tokio::task::JoinHandle;

/// A background task that sends heartbeats to the server.
pub struct HeartbeatLoop {
    handle: JoinHandle<()>,
}

impl HeartbeatLoop {
    pub fn start(sender: TypedSocketSender<MessageFromDrone>) -> Self {
        let handle = tokio::spawn(async move {
            loop {
                let local_time_epoch_millis = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("system time is before epoch")
                    .as_millis() as u64;
                if let Err(err) = sender.send(MessageFromDrone::Heartbeat {
                    local_time_epoch_millis,
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
