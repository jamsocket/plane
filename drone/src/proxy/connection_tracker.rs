use dashmap::{DashMap, DashSet};
use std::sync::Arc;

#[derive(Default)]
struct DashMultiset {
    map: DashMap<String, u32>,
}

impl DashMultiset {
    pub fn add(&self, backend: &str) {
        self.map
            .entry(backend.to_string())
            .and_modify(|d| *d += 1)
            .or_insert_with(|| 1);
    }

    pub fn remove(&self, backend: &str) {
        self.map.remove_if_mut(backend, |_, d| {
            *d -= 1;
            *d == 0
        });
    }
}

#[derive(Clone, Default)]
pub struct ConnectionTracker {
    request_events: Arc<DashSet<String>>,
    long_lived_connections: Arc<DashMultiset>,
}

impl ConnectionTracker {
    pub fn track_request(&self, backend: &str) {
        self.request_events.insert(backend.to_string());
    }

    pub fn increment_connections(&self, backend: &str) {
        self.long_lived_connections.add(backend);
    }

    pub fn decrement_connections(&self, backend: &str) {
        self.long_lived_connections.remove(backend);
    }

    pub fn get_and_clear_active_backends(&self) -> Vec<String> {
        let request_events = self.request_events.clone();

        for conn in &self.long_lived_connections.map {
            request_events.insert(conn.key().to_string());
        }

        let result = request_events.iter().map(|d| d.key().clone()).collect();
        self.request_events.clear();

        result
    }
}
