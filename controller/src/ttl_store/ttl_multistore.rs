use super::{ttl_list::TtlList, ttl_map::TtlMap};
use std::{
    hash::Hash,
    time::{Duration, SystemTime},
};

pub struct TtlMultistore<K: Hash + Eq + Clone, V> {
    inner: TtlMap<K, TtlList<V>>,
    ttl: Duration,
}

impl<K: Hash + Eq + Clone, V> TtlMultistore<K, V> {
    pub fn new(ttl: Duration) -> Self {
        TtlMultistore {
            ttl,
            inner: TtlMap::new(ttl),
        }
    }

    pub fn insert(&mut self, key: K, value: V, time: SystemTime) {
        let list = self.inner.get_or_insert(key, || TtlList::new(self.ttl));
        list.push(value, time);
    }

    pub fn iter(&mut self, key: &K, time: SystemTime) -> Option<impl Iterator<Item = &V>> {
        if let Some(v) = self.inner.get_mut(key, time) {
            Some(v.iter(time))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ttl_store::test::ts;

    #[test]
    fn test_multistore() {
        let mut store: TtlMultistore<u32, u32> = TtlMultistore::new(Duration::from_secs(10));

        store.insert(4, 10, ts(200));
        store.insert(4, 11, ts(201));
        store.insert(4, 12, ts(202));

        store.insert(5, 100, ts(205));
        store.insert(5, 110, ts(206));
        store.insert(5, 120, ts(207));

        let vals: Vec<u32> = store.iter(&4, ts(208)).unwrap().cloned().collect();
        assert_eq!(vec![10, 11, 12], vals);

        let vals: Vec<u32> = store.iter(&4, ts(211)).unwrap().cloned().collect();
        assert_eq!(vec![12], vals);

        let vals: Vec<u32> = store.iter(&5, ts(211)).unwrap().cloned().collect();
        assert_eq!(vec![100, 110, 120], vals);

        let vals: Vec<u32> = store.iter(&4, ts(212)).unwrap().cloned().collect();
        assert!(vals.is_empty());

        let vals: Vec<u32> = store.iter(&5, ts(215)).unwrap().cloned().collect();
        assert_eq!(vec![110, 120], vals);

        let vals: Vec<u32> = store.iter(&5, ts(216)).unwrap().cloned().collect();
        assert_eq!(vec![120], vals);

        let vals: Vec<u32> = store.iter(&5, ts(217)).unwrap().cloned().collect();
        assert!(vals.is_empty());
    }
}
