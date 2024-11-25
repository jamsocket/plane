use plane_client::names::BackendName;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContainerId(String);

impl From<String> for ContainerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&BackendName> for ContainerId {
    fn from(backend_id: &BackendName) -> Self {
        Self(format!("plane-{}", backend_id))
    }
}

impl ContainerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
#[cfg(test)]
mod tests {
    use plane_client::names::{BackendName, Name};

    #[test]
    fn test_backend_name_container_id_roundtrip() {
        let original_name = BackendName::new_random();
        let roundtrip_name =
            BackendName::try_from(original_name.to_string()).expect("Should convert back");
        assert_eq!(original_name, roundtrip_name);
    }
}
