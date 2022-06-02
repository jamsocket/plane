use serde::{Deserialize, Serialize};

use crate::nats::TypedMessage;

#[derive(Serialize, Deserialize, Debug)]
pub struct SetAcmeDnsRecord {
    pub cluster: String,
    pub value: String,
}

impl TypedMessage for SetAcmeDnsRecord {
    type Response = ();

    fn subject(&self) -> String {
        "acme.set_dns_record".to_string()
    }
}
