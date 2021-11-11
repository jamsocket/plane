use crate::parse_proc::{parse_connections, Port, TcpConnectionState};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::RwLock,
    time::SystemTime,
};

pub const LOCK_ERROR: &str = "state lock should never be poisoned, but was.";

/// Represents the connection state, both as internal representation and to be
/// serialized into JSON for the remote data consumer.
#[derive(Serialize, Clone)]
pub struct ConnectionState {
    active_connections: u32,
    waiting_connections: u32,
    seconds_inactive: u32,
    listening: bool,

    #[serde(skip_serializing)]
    first_inactive: Option<SystemTime>,
}

/// Utility for monitoring the connection on a given port. State is read from the
/// kernel upon construction and after that whenever `refresh()` is called.
pub struct ConnectionMonitor {
    state: RwLock<ConnectionState>,
    connections_file: PathBuf,
    monitor_port: u16,
}

impl ConnectionMonitor {
    /// Returns a clone of the current ConnectionState.
    pub fn state(&self) -> ConnectionState {
        self.state.read().expect(LOCK_ERROR).clone()
    }

    /// Counts the current “established” and “waiting” connections on the given
    /// (local) port, and returns them, along with an indicator of whether it
    /// found a process currently listening on that port.
    fn count_connections(port: u16, connections_file: &Path) -> (u32, u32, bool) {
        let mut buffer: Vec<u8> = Vec::new();
        File::open(connections_file)
            .expect("Could not open TCP connections file.")
            .read_to_end(&mut buffer)
            .expect("Could not read TCP connections file as bytes.");
        let connections =
            parse_connections(&buffer).expect("Could not parse TCP connections file.");

        let mut active_connections: u32 = 0;
        let mut waiting_connections: u32 = 0;
        let mut listening: bool = false;

        for connection in connections {
            if connection.local_address.port == Port::Port(port) {
                match connection.state {
                    TcpConnectionState::Established => {
                        active_connections += 1;
                    }
                    TcpConnectionState::TimeWait => {
                        waiting_connections += 1;
                    }
                    TcpConnectionState::Listen => {
                        listening = true;
                    }
                    state => {
                        log::info!("Unhandled connection state: {:?}", state);
                    }
                }
            }
        }

        (active_connections, waiting_connections, listening)
    }

    /// Construct a new connection monitor. Reads the connection state at
    /// construction time. Only updates when `refresh` is called.
    pub fn new(monitor_port: u16, connections_file: PathBuf) -> Self {
        let (active_connections, waiting_connections, listening) =
            Self::count_connections(monitor_port, &connections_file);

        let state = ConnectionState {
            active_connections,
            waiting_connections,
            seconds_inactive: 0,
            listening,
            first_inactive: None,
        };

        ConnectionMonitor {
            state: RwLock::new(state),
            connections_file,
            monitor_port,
        }
    }

    /// Update the current connection state.
    pub fn refresh(&self) {
        let (active_connections, waiting_connections, listening) =
            Self::count_connections(self.monitor_port, &self.connections_file);

        let now = SystemTime::now();

        log::info!(
            "Found {} active and {} waiting connections. Listening: {}",
            active_connections,
            waiting_connections,
            listening
        );

        let mut state = self.state.write().expect(LOCK_ERROR);

        let first_inactive = if active_connections > 0 || waiting_connections > 0 {
            None
        } else {
            state.first_inactive.or(Some(SystemTime::now()))
        };

        let seconds_inactive = if let Some(first_inactive) = first_inactive {
            now.duration_since(first_inactive)
                .map(|d| d.as_secs())
                .unwrap_or_default() as u32
        } else {
            0
        };

        *state = ConnectionState {
            active_connections,
            waiting_connections,
            seconds_inactive,
            listening,
            first_inactive,
        };
    }
}
