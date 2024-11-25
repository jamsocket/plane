use plane_client::names::BackendName;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tokio::task::JoinHandle;

use crate::heartbeat_consts::HEARTBEAT_INTERVAL;

type BackendNameListener = Box<dyn Fn(&BackendName) + Send + Sync + 'static>;

#[derive(Debug, Clone)]
pub struct BackendEntry {
    /// The current number of active connections to the backend.
    pub active_connections: u32,

    /// Whether the backend has had a recent connection (since this value was last checked).
    pub had_recent_connection: bool,
}

#[derive(Default)]
pub struct ConnectionMonitor {
    visit_queue: VecDeque<(SystemTime, BackendName)>,
    backends: HashMap<BackendName, BackendEntry>,
    listener: Option<BackendNameListener>,
}

impl ConnectionMonitor {
    pub fn set_listener<F>(&mut self, listener: F)
    where
        F: Fn(&BackendName) + Send + Sync + 'static,
    {
        self.listener = Some(Box::new(listener));
    }

    pub fn touch_backend(&mut self, backend_id: &BackendName) {
        match self.backends.entry(backend_id.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().had_recent_connection = true;
            }
            Entry::Vacant(entry) => {
                if let Some(listener) = &self.listener {
                    listener(backend_id);
                }
                self.visit_queue
                    .push_back((SystemTime::now() + HEARTBEAT_INTERVAL, backend_id.clone()));
                entry.insert(BackendEntry {
                    active_connections: 0,
                    had_recent_connection: true,
                });
            }
        }
    }

    pub fn inc_connection(&mut self, backend_id: &BackendName) {
        match self.backends.entry(backend_id.clone()) {
            Entry::Occupied(mut entry) => {
                let backend_entry = entry.get_mut();
                backend_entry.active_connections += 1;
                backend_entry.had_recent_connection = true;
            }
            Entry::Vacant(entry) => {
                if let Some(listener) = &self.listener {
                    listener(backend_id);
                }

                self.visit_queue
                    .push_back((SystemTime::now() + HEARTBEAT_INTERVAL, backend_id.clone()));
                entry.insert(BackendEntry {
                    active_connections: 1,
                    had_recent_connection: true,
                });
            }
        }
    }

    pub fn dec_connection(&mut self, backend_id: &BackendName) {
        match self.backends.entry(backend_id.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().active_connections -= 1;
            }
            Entry::Vacant(_) => {
                tracing::warn!(
                    "ConnectionMonitor::remove_connection called for unknown backend: {:?}",
                    backend_id
                );
            }
        }
    }
}

pub struct ConnectionMonitorHandle {
    monitor: Arc<Mutex<ConnectionMonitor>>,
    handle: JoinHandle<()>,
}

#[allow(clippy::new_without_default)]
impl ConnectionMonitorHandle {
    pub fn new() -> Self {
        let monitor: Arc<Mutex<ConnectionMonitor>> = Arc::default();

        let handle = {
            let monitor = monitor.clone();
            tokio::spawn(async move {
                loop {
                    let Some((time, backend)) = monitor
                        .lock()
                        .expect("Monitor lock was poisoned.")
                        .visit_queue
                        .pop_front()
                    else {
                        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                        continue;
                    };

                    let now = SystemTime::now();

                    if now < time {
                        tokio::time::sleep(
                            time.duration_since(now)
                                .expect("Always able to convert finite duration."),
                        )
                        .await;
                    }

                    let mut monitor_lock = monitor.lock().expect("Monitor lock was poisoned.");

                    let Some(backend_entry) = monitor_lock.backends.get_mut(&backend) else {
                        // This shouldn't happen.
                        continue;
                    };

                    if backend_entry.active_connections > 0 || backend_entry.had_recent_connection {
                        backend_entry.had_recent_connection = false;

                        if let Some(listener) = &monitor_lock.listener {
                            listener(&backend);
                        }

                        monitor_lock
                            .visit_queue
                            .push_back((SystemTime::now() + HEARTBEAT_INTERVAL, backend));
                    } else {
                        // The backend has no connections and has not been touched recently, so we can remove it.
                        monitor_lock.backends.remove(&backend);
                    }
                }
            })
        };
        Self { monitor, handle }
    }

    pub fn get_backend_entry(&self, backend_id: &BackendName) -> Option<BackendEntry> {
        self.monitor
            .lock()
            .expect("Monitor lock was poisoned.")
            .backends
            .get(backend_id)
            .cloned()
    }

    pub fn monitor(&self) -> Arc<Mutex<ConnectionMonitor>> {
        self.monitor.clone()
    }

    pub fn set_listener<F>(&self, listener: F)
    where
        F: Fn(&BackendName) + Send + Sync + 'static,
    {
        self.monitor
            .lock()
            .expect("Monitor lock was poisoned.")
            .set_listener(listener);
    }

    pub fn touch_backend(&self, backend_id: &BackendName) {
        self.monitor
            .lock()
            .expect("Monitor lock was poisoned")
            .touch_backend(backend_id);
    }
}

impl Drop for ConnectionMonitorHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
