use crate::nats::{Subject, SubscribeSubject, TypedMessage};
use serde::{Deserialize, Serialize};

/// A request from the drone to the DNS server telling it to set
/// a TXT record on the given domain with the given value.
#[derive(Serialize, Deserialize, Debug)]
pub struct SetAcmeDnsRecord {
    pub cluster: String,
    pub value: String,
}

impl TypedMessage for SetAcmeDnsRecord {
    type Response = bool;

    fn subject(&self) -> Subject<Self> {
        Subject::new("acme.set_dns_record".to_string())
    }
}

impl SetAcmeDnsRecord {
    pub fn subscribe_subject() -> SubscribeSubject<Self> {
        SubscribeSubject::new("acme.set_dns_record".to_string())
    }
}
