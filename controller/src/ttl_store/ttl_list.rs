use std::{
    collections::VecDeque,
    time::{Duration, SystemTime},
};

pub struct TtlList<V> {
    items: VecDeque<(SystemTime, V)>,
    ttl: Duration,
    last_compaction: SystemTime,
}

impl<V> TtlList<V> {
    pub fn new(ttl: Duration) -> Self {
        TtlList {
            ttl,
            items: VecDeque::new(),
            last_compaction: SystemTime::UNIX_EPOCH,
        }
    }

    pub fn iter(&mut self, time: SystemTime) -> impl Iterator<Item = &V> {
        self.compact(time);

        self.items.iter().map(|d| &d.1)
    }

    pub fn push(&mut self, value: V, time: SystemTime) {
        if time < self.last_compaction {
            tracing::info!(
                ?time,
                last_compaction=?self.last_compaction,
                "TtlStore received insertion request out of order."
            );
        }
        let expiry = time
            .checked_add(self.ttl)
            .expect("Adding ttl should never fail.");
        self.items.push_back((expiry, value));
    }

    fn compact(&mut self, time: SystemTime) {
        while let Some((t, _)) = self.items.front() {
            if *t > time {
                break;
            }

            self.items.pop_front();
        }

        self.last_compaction = time;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ttl_store::test::ts;

    #[test]
    fn test_list() {
        let mut list: TtlList<u32> = TtlList::new(Duration::from_secs(10));

        list.push(4, ts(100));
        list.push(5, ts(101));
        list.push(6, ts(102));
        list.push(7, ts(103));

        let vals: Vec<u32> = list.iter(ts(108)).cloned().collect();
        assert_eq!(vec![4, 5, 6, 7], vals);

        let vals: Vec<u32> = list.iter(ts(112)).cloned().collect();
        assert_eq!(vec![7], vals);

        let vals: Vec<u32> = list.iter(ts(113)).cloned().collect();
        assert!(vals.is_empty());
    }
}
