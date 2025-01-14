use plane_common::names::{BackendName, NameError};
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
        ContainerId(backend_id.to_container_id())
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

impl TryFrom<ContainerId> for BackendName {
    type Error = NameError;

    fn try_from(container_id: ContainerId) -> Result<Self, NameError> {
        BackendName::from_container_id(container_id.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::drone::runtime::docker::types::ContainerId;
    use plane_common::names::{BackendName, Name};

    #[test]
    fn test_backend_name_container_id_roundtrip() {
        let original_name = BackendName::new_random();
        let container_id: ContainerId = ContainerId::from(&original_name);
        let roundtrip_name = BackendName::try_from(container_id).expect("Should convert back");
        assert_eq!(original_name, roundtrip_name);
    }
}
