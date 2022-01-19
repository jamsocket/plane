use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use hyper::body::Bytes;
use tokio::sync::broadcast::{channel, Sender};
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    StreamExt,
};

#[derive(Clone)]
pub struct MonitorState {
    last_active: u64,
    live_connections: u32,
}

impl Into<Bytes> for MonitorState {
    fn into(self) -> Bytes {
        format!(
            "{{\"last_active\": {}, \"live_connections\": {}}}\n",
            self.last_active, self.live_connections
        )
        .into()
    }
}

pub struct Monitor {
    last_connection: AtomicU64,
    live_connections: AtomicU32,
    sender: Sender<MonitorState>,
}

fn time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Unexpectedly traveled through time.")
        .as_secs()
}

impl Monitor {
    pub fn new() -> Monitor {
        let (sender, _) = channel(16);

        Monitor {
            last_connection: AtomicU64::new(0),
            live_connections: AtomicU32::new(0),
            sender,
        }
    }

    pub fn state(&self) -> MonitorState {
        let live_connections = self.live_connections.load(Ordering::Relaxed);
        let last_active = self.last_connection.load(Ordering::Relaxed);

        MonitorState {
            live_connections,
            last_active,
        }
    }

    pub fn open_connection(&self) {
        self.live_connections.fetch_add(1, Ordering::Relaxed);

        self.bump();
    }

    pub fn close_connection(&self) {
        self.live_connections.fetch_sub(1, Ordering::Relaxed);

        self.bump();
    }

    pub fn bump(&self) {
        self.last_connection
            .fetch_max(time_now(), Ordering::Relaxed);

        let _ = self.sender.send(self.state());
    }

    /// Return an infinite stream of MonitorState events.
    ///
    /// The first state event will be the current state, and will be immediately available.
    /// Subsequent updates will be sent whenever the state changes.
    pub fn status_stream(
        &self,
    ) -> impl futures_core::Stream<Item = Result<MonitorState, BroadcastStreamRecvError>> {
        // Create a stream by taking the current state and wrapping it in a stream.
        let current_state = tokio_stream::once(Ok(self.state()));

        // Merge subsequent updates (which will come through our broadcast channel) to the stream.
        current_state.merge(BroadcastStream::new(self.sender.subscribe()))
    }
}
