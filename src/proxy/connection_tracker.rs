// use dashmap::DashMap;
// #[derive(Default)]
// struct DashMultiset {
//     map: DashMap<String, u32>,
// }

// impl DashMultiset {
//     pub fn add(&self, backend: &str) {
//         self.map
//             .entry(backend.to_string())
//             .and_modify(|d| *d += 1)
//             .or_insert_with(|| 1);
//     }

//     pub fn remove(&self, backend: &str) {
//         self.map.remove_if_mut(backend, |_, d| {
//             *d -= 1;
//             *d == 0
//         });
//     }
// }

use std::sync::Arc;

use dashmap::DashSet;

#[derive(Clone, Default)]
pub struct ConnectionTracker {
    request_events: Arc<DashSet<String>>,
}

impl ConnectionTracker {
    pub fn track_request(&self, backend: &str) {
        self.request_events.insert(backend.to_string());
    }

    pub fn get_and_clear_active_backends(&self) -> Vec<String> {
        let result = self.request_events.iter().map(|d| d.key().clone()).collect();
        self.request_events.clear();

        result
    }
}
