use crate::heartbeat_consts::HEARTBEAT_INTERVAL;
use chrono::Utc;
use plane_client::{log_types::LoggableTime, protocol::Heartbeat, typed_socket::TypedSocketSender};
use tokio::task::JoinHandle;

/// A background task that sends heartbeats to the server.
pub struct HeartbeatLoop {
    handle: JoinHandle<()>,
}

impl HeartbeatLoop {
    pub fn start(sender: TypedSocketSender<Heartbeat>) -> Self {
        let handle = tokio::spawn(async move {
            loop {
                let local_time = LoggableTime(Utc::now());
                if let Err(err) = sender.send(Heartbeat { local_time }) {
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
