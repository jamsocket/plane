pub mod ttl_list;
pub mod ttl_map;
pub mod ttl_multistore;

#[cfg(test)]
mod test {
    use chrono::{DateTime, NaiveDateTime, Utc};

    pub fn ts(timestamp: i64) -> DateTime<Utc> {
        DateTime::from_utc(NaiveDateTime::from_timestamp(timestamp, 0), Utc)
    }
}
