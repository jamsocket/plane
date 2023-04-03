use crate::{nats::TypedMessage, types::ClusterName};
use serde::{Deserialize, Serialize};

/// A request from the drone to the DNS server telling it to set
/// a TXT record on the given domain with the given value.
#[derive(Serialize, Deserialize, Debug)]
pub struct SetAcmeDnsRecord {
    pub cluster: ClusterName,
    pub value: String,
}

impl TypedMessage for SetAcmeDnsRecord {
    type Response = bool;

    fn subject(&self) -> String {
        format!("cluster.{}.set_acme_record", self.cluster.subject_name())
    }
}
