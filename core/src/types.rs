use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::{convert::Infallible, fmt::Display, str::FromStr};
use uuid::Uuid;

const RESOURCE_PREFIX: &str = "plane-";
const MAX_RESOURCE_LOCK_LENGTH_BYTES: usize = 2048;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DroneId(String);

impl Display for DroneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl DroneId {
    #[must_use]
    pub fn new(id: String) -> Self {
        DroneId(id)
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn new_random() -> Self {
        let id = Uuid::new_v4();
        DroneId(id.to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BackendId(String);

impl Display for BackendId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub trait AsSubjectComponent {
    fn as_subject_component(&self) -> String;
}

impl AsSubjectComponent for BackendId {
    fn as_subject_component(&self) -> String {
        self.id().into()
    }
}

impl AsSubjectComponent for DroneId {
    fn as_subject_component(&self) -> String {
        self.id().into()
    }
}

impl AsSubjectComponent for ClusterName {
    fn as_subject_component(&self) -> String {
        self.subject_name()
    }
}

impl BackendId {
    #[must_use]
    pub fn new(id: String) -> Self {
        BackendId(id)
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn to_resource_name(&self) -> String {
        format!("{}{}", RESOURCE_PREFIX, self.0)
    }

    #[must_use]
    pub fn from_resource_name(resource_name: &str) -> Option<Self> {
        resource_name
            .strip_prefix(RESOURCE_PREFIX)
            .map(|d| BackendId(d.to_string()))
    }

    #[must_use]
    pub fn new_random() -> Self {
        let id = Uuid::new_v4();
        BackendId(id.to_string())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LockState {
    Unlocked,
    Announced { uid: u64 },
    Assigned { backend: BackendId },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClusterName(String);

impl FromStr for ClusterName {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // TODO: we could ensure validity here.
        Ok(ClusterName::new(s))
    }
}

impl ClusterName {
    pub fn new(name: &str) -> Self {
        ClusterName(name.to_string())
    }

    pub fn hostname(&self) -> &str {
        &self.0
    }

    pub fn subject_name(&self) -> String {
        self.0.replace('.', "_")
    }
}

impl Display for ClusterName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceLock(String);

impl TryFrom<String> for ResourceLock {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl AsRef<str> for ResourceLock {
    fn as_ref(&self) -> &str {
        self.lock()
    }
}

impl Display for ResourceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.lock().fmt(f)
    }
}

impl AsSubjectComponent for ResourceLock {
    fn as_subject_component(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.lock().hash(&mut hasher);
        hasher.finish().to_string()
    }
}

impl ResourceLock {
    pub fn try_new(lock: String) -> Result<Self, anyhow::Error> {
        let lock_len = lock.as_bytes().len();
        if lock_len > MAX_RESOURCE_LOCK_LENGTH_BYTES {
            return Err(anyhow::anyhow!(format!(
                "Lock too long! max length allowed: {}, length of lock supplied: {}",
                MAX_RESOURCE_LOCK_LENGTH_BYTES, lock_len
            )));
        }
        Ok(ResourceLock(lock))
    }

    #[must_use]
    pub fn lock(&self) -> &str {
        &self.0
    }
}
