use serde::{Deserialize, Serialize};

use crate::nats::Subject;

/// A request from the drone to the DNS server telling it to set
/// a TXT record on the given domain with the given value.
#[derive(Serialize, Deserialize, Debug)]
pub struct SetAcmeDnsRecord {
    pub cluster: String,
    pub value: String,
}

impl SetAcmeDnsRecord {
    #[must_use] pub fn subject() -> Subject<SetAcmeDnsRecord, bool> {
        Subject::new("acme.set_dns_record".to_string())
    }
}
