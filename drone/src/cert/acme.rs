use anyhow::Result;
use base64::Engine;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Serialize, Deserialize, Clone)]
pub struct AcmeEabConfiguration {
    pub key_id: String,

    #[serde(serialize_with = "as_base64", deserialize_with = "from_base64")]
    pub key: Vec<u8>,
}

impl AcmeEabConfiguration {
    pub fn new(key_id: &str, key_b64: &str) -> Result<Self> {
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(key_b64)?;
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
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
    serializer.serialize_str(&enc)
}

fn from_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let result = String::deserialize(deserializer)?;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(result)
        .map_err(|err| Error::custom(format!("Error decoding base64 string: {}", err)))
}

impl AcmeEabConfiguration {
    pub fn eab_key_b64(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.key)
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_parse_base64() {
        let value = json!({
            "key_id": "key-id",
            "key": "ZmRzYWppbw"
        });
        let result: AcmeEabConfiguration = serde_json::from_value(value).unwrap();
        assert_eq!(result.key_id, "key-id");
        assert_eq!(result.key, b"fdsajio");
    }
}
