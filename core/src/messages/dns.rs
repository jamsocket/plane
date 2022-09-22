use crate::{
    nats::{SubscribeSubject, TypedMessage},
    types::ClusterName,
};
use serde::{Deserialize, Serialize};
use trust_dns_server::client::rr::RecordType;

#[derive(Serialize, Deserialize, Debug)]
pub enum LocalDnsRecordType {
    A,
    TXT,
    AAAA,
    Unknown(u16),
}

impl LocalDnsRecordType {
    fn as_trust_record_type(&self) -> RecordType {
        match self {
            LocalDnsRecordType::A => RecordType::A,
            LocalDnsRecordType::TXT => RecordType::TXT,
            LocalDnsRecordType::AAAA => RecordType::AAAA,
            LocalDnsRecordType::Unknown(id) => RecordType::Unknown(*id),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SetDnsRecord {
    pub cluster: ClusterName,
    pub kind: LocalDnsRecordType,
    pub value: String,
}

impl TypedMessage for SetDnsRecord {
    type Response = bool;

    fn subject(&self) -> String {
        format!(
            "cluster.{}.dns.{}",
            self.cluster,
            self.kind.as_trust_record_type()
        )
    }
}

impl SetDnsRecord {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.dns.*".into())
    }
}
