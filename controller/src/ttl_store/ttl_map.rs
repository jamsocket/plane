use chrono::{DateTime, Duration, Utc};
use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

/// A HashMap which enforces a time-to-live of records inserted
/// into it.
///
/// This is purely a data structure and does not do its own timekeeping;
/// times are instead passed in as an argument to each method.
///
/// Compaction happens automatically when values are read or written.
/// As a consequence of this, even reads require the data structure to
/// be mutable.
pub struct TtlMap<K: Hash + Eq + Clone, V> {
    ttl: Duration,
    inner_map: HashMap<K, (DateTime<Utc>, V)>,
    queue: VecDeque<(DateTime<Utc>, K)>,
    last_time: DateTime<Utc>,
}

impl<K: Hash + Eq + Clone, V> TtlMap<K, V> {
    pub fn new(ttl: Duration) -> Self {
        TtlMap {
            ttl,
            inner_map: HashMap::new(),
            queue: VecDeque::new(),
            last_time: DateTime::<Utc>::MIN_UTC,
        }
    }

    pub fn insert(&mut self, key: K, value: V, time: DateTime<Utc>) {
        if time < self.last_time {
            tracing::info!(
                %time,
                last_time=%self.last_time,
                "TtlStore received insertion request out of order."
            );
        }
        let expiry = time
            .checked_add_signed(self.ttl)
            .expect("Adding ttl should never fail.");
        self.inner_map.insert(key.clone(), (expiry, value));
        self.queue.push_back((expiry, key));
    }

    pub fn get_or_insert<F: FnOnce() -> V>(&mut self, key: K, func: F) -> &mut V {
        let now = Utc::now();
        let result = self.inner_map.entry(key).or_insert_with(|| (now, func()));
        &mut result.1
    }

    pub fn get(&mut self, key: &K, time: DateTime<Utc>) -> Option<&V> {
        self.compact(time);

        self.inner_map.get(key).map(|d| &d.1)
    }

    pub fn get_mut(&mut self, key: &K, time: DateTime<Utc>) -> Option<&mut V> {
        self.compact(time);

        self.inner_map.get_mut(key).map(|d| &mut d.1)
    }

    fn compact(&mut self, time: DateTime<Utc>) {
        if time < self.last_time {
            tracing::info!(
                %time,
                last_time=%self.last_time,
                "TtlStore received compaction request out of order."
            );
        }

        while let Some((t, _)) = self.queue.front() {
            if *t >= time {
                return;
            }

            let (_, key) = self
                .queue
                .pop_front()
                .expect("queue should never be empty here.");

            if let Some((current_expiry, _)) = self.inner_map.get(&key) {
                if *current_expiry < time {
                    self.inner_map.remove(&key);
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ttl_store::test::ts;

    #[test]
    fn test_ttl_store() {
        let mut store: TtlMap<String, String> = TtlMap::new(Duration::seconds(10));

        assert_eq!(0, store.inner_map.len());
        store.insert("foo".into(), "bar".into(), ts(10));
        assert_eq!(1, store.inner_map.len());
        assert_eq!(
            Some(&"bar".to_string()),
            store.get(&"foo".to_string(), ts(11))
        );

        store.compact(ts(25));
        assert_eq!(0, store.inner_map.len());
        assert_eq!(None, store.get(&"foo".to_string(), ts(25)));
    }

    #[test]
    fn test_ttl_store_ignores_overwrite() {
        let mut store: TtlMap<String, String> = TtlMap::new(Duration::seconds(10));

        assert_eq!(0, store.inner_map.len());
        store.insert("foo".into(), "bar".into(), ts(10));
        assert_eq!(
            Some(&"bar".to_string()),
            store.get(&"foo".to_string(), ts(11))
        );
        store.insert("foo".into(), "baz".into(), ts(19));
        store.compact(ts(21));
        assert_eq!(
            1,
            store.inner_map.len(),
            "Value was erroneously evicted even though it has been touched."
        );

        assert_eq!(
            Some(&"baz".to_string()),
            store.get(&"foo".to_string(), ts(25))
        );
    }

    #[test]
    fn test_multiple_keys() {
        let mut store: TtlMap<String, String> = TtlMap::new(Duration::seconds(10));

        assert_eq!(0, store.inner_map.len());
        store.insert("foo1".into(), "bar1".into(), ts(10));
        store.insert("foo2".into(), "bar2".into(), ts(12));
        store.insert("foo3".into(), "bar3".into(), ts(14));

        assert_eq!(
            Some(&"bar1".to_string()),
            store.get(&"foo1".to_string(), ts(15))
        );
        assert_eq!(
            Some(&"bar1".to_string()),
            store.get(&"foo1".to_string(), ts(20))
        );
        assert_eq!(None, store.get(&"foo1".to_string(), ts(21)));
        assert_eq!(
            Some(&"bar2".to_string()),
            store.get(&"foo2".to_string(), ts(22))
        );
        assert_eq!(
            Some(&"bar3".to_string()),
            store.get(&"foo3".to_string(), ts(24))
        );
        assert_eq!(None, store.get(&"foo1".to_string(), ts(33)));
        assert_eq!(None, store.get(&"foo2".to_string(), ts(33)));
        assert_eq!(None, store.get(&"foo3".to_string(), ts(33)));
    }
}
