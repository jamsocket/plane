pub mod ttl_list;
pub mod ttl_map;
pub mod ttl_multistore;

#[cfg(test)]
mod test {
    use std::time::{Duration, SystemTime};

    pub fn ts(timestamp: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp)
    }
}
