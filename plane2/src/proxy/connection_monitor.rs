use crate::{heartbeat_consts::HEARTBEAT_INTERVAL_SECONDS, names::BackendName};
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};
use tokio::task::JoinHandle;

type BackendNameListener = Box<dyn Fn(&BackendName) + Send + Sync + 'static>;

#[derive(Default)]
pub struct ConnectionMonitor {
    visit_queue: VecDeque<(SystemTime, BackendName)>,
    backends: HashMap<BackendName, (AtomicU32, AtomicBool)>,
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
            Entry::Occupied(entry) => {
                entry.get().1.store(true, Ordering::Relaxed);
            }
            Entry::Vacant(entry) => {
                if let Some(listener) = &self.listener {
                    listener(backend_id);
                }
                self.visit_queue.push_back((
                    SystemTime::now() + Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS as _),
                    backend_id.clone(),
                ));
                entry.insert((AtomicU32::new(0), AtomicBool::new(false)));
            }
        }
    }

    pub fn inc_connection(&mut self, backend_id: &BackendName) {
        match self.backends.entry(backend_id.clone()) {
            Entry::Occupied(entry) => {
                let (active_connections, had_recent_connection) = entry.get();
                active_connections.fetch_add(1, Ordering::SeqCst);
                had_recent_connection.store(true, Ordering::SeqCst);
            }
            Entry::Vacant(entry) => {
                if let Some(listener) = &self.listener {
                    listener(backend_id);
                }

                self.visit_queue.push_back((
                    SystemTime::now() + Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS as _),
                    backend_id.clone(),
                ));
                entry.insert((AtomicU32::new(1), AtomicBool::new(false)));
            }
        }
    }

    pub fn dec_connection(&mut self, backend_id: &BackendName) {
        match self.backends.entry(backend_id.clone()) {
            Entry::Occupied(entry) => {
                entry.get().0.fetch_sub(1, Ordering::SeqCst);
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
                        tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS as _))
                            .await;
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

                    let Some((connection_count, recent_connection)) =
                        monitor_lock.backends.get(&backend)
                    else {
                        // This shouldn't happen.
                        continue;
                    };

                    if connection_count.load(Ordering::SeqCst) == 0
                        && !recent_connection.load(Ordering::Relaxed)
                    {
                        // The backend has no connections and has not been touched recently, so we can remove it.
                        monitor_lock.backends.remove(&backend);
                    } else {
                        recent_connection.store(false, Ordering::Relaxed);

                        if let Some(listener) = &monitor_lock.listener {
                            listener(&backend);
                        }

                        monitor_lock.visit_queue.push_back((
                            SystemTime::now()
                                + Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS as _),
                            backend,
                        ));
                    }
                }
            })
        };
        Self { monitor, handle }
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
