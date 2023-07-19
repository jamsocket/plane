use serde::{Deserialize, Serialize};
use std::{convert::Infallible, fmt::Display, str::FromStr};
use uuid::Uuid;

const RESOURCE_PREFIX: &str = "plane-";

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

pub type PlaneLockName = String;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlaneLockState {
    Unlocked,
    Announced { uid: u128 },
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
