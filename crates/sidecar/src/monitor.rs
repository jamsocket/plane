use hyper::body::Bytes;
use std::{
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast::{channel, Sender};
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    StreamExt,
};

#[derive(Clone)]
pub enum MonitorState {
    NotReady,
    LiveConnections(u32),
    Inactive(u32),
}

impl Into<Bytes> for MonitorState {
    fn into(self) -> Bytes {
        match self {
            MonitorState::NotReady => format!("data: {{\"ready\": false}}\n\n").into(),
            MonitorState::LiveConnections(connections) => format!(
                "data: {{\"ready\": true, \"live_connections\": {}}}\n\n",
                connections
            )
            .into(),
            MonitorState::Inactive(seconds_since_active) => format!(
                "data: {{\"ready\": true, \"seconds_since_active\": {}}}\n\n",
                seconds_since_active
            )
            .into(),
        }
    }
}

pub struct Monitor {
    last_connection: AtomicU64,
    live_connections: AtomicU32,
    sender: Sender<MonitorState>,
    ready: AtomicBool,
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
            last_connection: AtomicU64::new(time_now()),
            live_connections: AtomicU32::new(0),
            sender,
            ready: AtomicBool::new(false),
        }
    }

    pub fn set_ready(&self) {
        self.ready.store(true, Ordering::Relaxed);
        self.bump();
    }

    pub fn state(&self) -> MonitorState {
        if !self.ready.load(Ordering::Relaxed) {
            return MonitorState::NotReady;
        }

        let live_connections = self.live_connections.load(Ordering::Relaxed);
        let last_active = self.last_connection.load(Ordering::Relaxed);

        if live_connections > 0 {
            MonitorState::LiveConnections(live_connections)
        } else {
            // max() because SystemTime can theoretically decrease over short durations.
            let seconds_since_active = time_now().max(last_active) - last_active;
            MonitorState::Inactive(seconds_since_active as u32)
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
