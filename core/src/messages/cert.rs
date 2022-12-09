use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject, TypedMessage},
    types::ClusterName,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A request from the drone to the DNS server telling it to set
/// a TXT record on the given domain with the given value.
#[derive(Serialize, Deserialize, Debug)]
pub struct SetAcmeDnsRecord {
    pub cluster: ClusterName,
    pub value: String,
}

impl TypedMessage for SetAcmeDnsRecord {
    type Response = NoReply;

    fn subject(&self) -> String {
        "acme.set_dns_record".to_string()
    }
}

impl SetAcmeDnsRecord {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("acme.set_dns_record".to_string())
    }

    pub fn ttl() -> Duration {
        Duration::from_secs(60)
    }
}

impl JetStreamable for SetAcmeDnsRecord {
    fn stream_name() -> &'static str {
        "acme_set_dns_record"
    }

    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().to_string(),
            subjects: vec!["acme.set_dns_record".to_string()],
            max_age: Self::ttl(),
            ..Default::default()
        }
    }
}
