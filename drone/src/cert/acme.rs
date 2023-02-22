use anyhow::Result;
use base64::Engine;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Serialize, Deserialize)]
pub struct AcmeEabConfiguration {
    pub key_id: String,

    #[serde(serialize_with = "as_base64", deserialize_with = "from_base64")]
    pub key: Vec<u8>,
}

impl AcmeEabConfiguration {
    pub fn new(key_id: &str, key_b64: &str) -> Result<Self> {
        let key = base64::engine::general_purpose::URL_SAFE.decode(key_b64)?;
        Ok(AcmeEabConfiguration {
            key_id: key_id.into(),
            key,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct AcmeConfiguration {
    pub server: String,
    pub admin_email: String,
    pub eab: Option<AcmeEabConfiguration>,
}

fn as_base64<S>(key: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let enc = base64::engine::general_purpose::URL_SAFE.encode(key);
    serializer.serialize_str(&enc)
}

fn from_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let result = String::deserialize(deserializer)?;
    base64::engine::general_purpose::URL_SAFE
        .decode(result)
        .map_err(|err| Error::custom(err.to_string()))
}

impl AcmeEabConfiguration {
    pub fn eab_key_b64(&self) -> String {
        base64::engine::general_purpose::URL_SAFE.encode(&self.key)
    }
}
