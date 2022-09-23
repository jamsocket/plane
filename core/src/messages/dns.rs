use crate::{
    nats::{SubscribeSubject, TypedMessage},
    types::ClusterName,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum LocalDnsRecordType {
    A,
    TXT,
    AAAA,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SetDnsRecord {
    pub cluster: ClusterName,
    pub kind: LocalDnsRecordType,
    pub name: String,
    pub value: String,
}

impl TypedMessage for SetDnsRecord {
    type Response = bool;

    fn subject(&self) -> String {
        format!(
            "cluster.{}.dns.{}",
            self.cluster,
            serde_json::to_string(&self.kind).unwrap()
        )
    }
}

impl SetDnsRecord {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("cluster.*.dns.*".into())
    }
}
