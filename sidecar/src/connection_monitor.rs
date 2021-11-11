use crate::parse_proc::{parse_connections, Port, TcpConnectionState};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::SystemTime,
};

pub const LOCK_ERROR: &str = "state lock should never be poisoned, but was.";

/// Represents the connection state, both as internal representation and to be
/// serialized into JSON for the remote data consumer.
#[derive(Serialize, Clone)]
pub struct ConnectionState {
    active_connections: u32,
    waiting_connections: u32,
    seconds_since_active: u32,
    listening: bool,
}

/// Utility for monitoring the connection on a given port. State is read from the
/// kernel upon construction and after that whenever `refresh()` is called.
pub struct ConnectionMonitor {
    state: Arc<RwLock<ConnectionState>>,
    last_active: SystemTime,
    connections_file: PathBuf,
    monitor_port: u16,
}

impl ConnectionMonitor {
    pub fn state(&self) -> ConnectionState {
        self.state.read().expect(LOCK_ERROR).clone()
    }

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

    pub fn new(monitor_port: u16, connections_file: PathBuf) -> Self {
        let (active_connections, waiting_connections, listening) =
            Self::count_connections(monitor_port, &connections_file);

        let state = ConnectionState {
            active_connections,
            waiting_connections,
            seconds_since_active: 0,
            listening,
        };

        ConnectionMonitor {
            state: Arc::new(RwLock::new(state)),
            last_active: SystemTime::now(),
            connections_file,
            monitor_port,
        }
    }

    pub fn refresh(&mut self) {
        let (active_connections, waiting_connections, listening) =
            Self::count_connections(self.monitor_port, &self.connections_file);

        let now = SystemTime::now();

        if active_connections > 0 || waiting_connections > 0 {
            self.last_active = now;
        }

        log::info!(
            "Found {} active and {} waiting connections. Listening: {}",
            active_connections,
            waiting_connections,
            listening
        );

        let seconds_since_active = now
            .duration_since(self.last_active)
            .map(|d| d.as_secs())
            .unwrap_or_default() as u32;

        *self.state.write().expect(LOCK_ERROR) = ConnectionState {
            active_connections,
            waiting_connections,
            seconds_since_active,
            listening,
        };
    }
}
