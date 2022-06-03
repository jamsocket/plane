use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DroneId(u32);

impl DroneId {
    pub fn id(&self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BackendId(String);

impl BackendId {
    pub fn id(&self) -> &str {
        &self.0
    }
}
