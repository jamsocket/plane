use crate::{
    nats::{JetStreamable, NoReply, SubscribeSubject, TypedMessage},
    types::ClusterName,
};
use serde::{Deserialize, Serialize};
use std::{fmt::Display, time::Duration};

/// Number of seconds “early” that a message with a TTL should be
/// re-sent, to account for network delay and variance.
const TTL_BUFFER_SECONDS: u64 = 10;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum DnsRecordType {
    A,
    TXT,
}

impl Display for DnsRecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// **DEPRECATED**. Will be removed in a future version of Plane.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct SetDnsRecord {
    pub cluster: ClusterName,
    pub kind: DnsRecordType,
    pub name: String,
    pub value: String,
}

impl TypedMessage for SetDnsRecord {
    type Response = NoReply;

    fn subject(&self) -> String {
        format!("cluster.{}.dns.{}", self.cluster.subject_name(), self.kind)
    }
}

impl JetStreamable for SetDnsRecord {
    fn config() -> async_nats::jetstream::stream::Config {
        async_nats::jetstream::stream::Config {
            name: Self::stream_name().into(),
            subjects: vec!["cluster.*.dns.*".into()],
            max_age: Duration::from_secs(Self::ttl_seconds()),
            ..async_nats::jetstream::stream::Config::default()
        }
    }

    fn stream_name() -> &'static str {
        "dns_record"
    }
}

impl SetDnsRecord {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.dns.*".into())
    }

    fn ttl_seconds() -> u64 {
        60
    }

    pub fn ttl() -> Duration {
        Duration::from_secs(Self::ttl_seconds())
    }

    pub fn send_period() -> u64 {
        Self::ttl_seconds() - TTL_BUFFER_SECONDS
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_set_dns_record_subject() {
        let record = SetDnsRecord {
            cluster: ClusterName::new("foo.bar"),
            kind: DnsRecordType::A,
            name: "blah".to_string(),
            value: "12.12.12.12".to_string(),
        };

        assert_eq!("cluster.foo_bar.dns.A", &record.subject());

        let record = SetDnsRecord {
            cluster: ClusterName::new("gad.wom.tld"),
            kind: DnsRecordType::TXT,
            name: "goo".to_string(),
            value: "14.14.14.14".to_string(),
        };

        assert_eq!("cluster.gad_wom_tld.dns.TXT", &record.subject());
    }
}
