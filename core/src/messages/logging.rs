use crate::nats::{NoReply, TypedMessage};
use crate::types::DroneId;
use anyhow::Result;
use chrono::{DateTime, Utc};
use core::str::FromStr;
use serde::de::{Error, Unexpected};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::Level;

#[derive(Debug)]
pub struct SerializableLevel(pub Level);

impl Serialize for SerializableLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        str::serialize(self.0.as_str(), serializer)
    }
}

impl<'de> Deserialize<'de> for SerializableLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let level = String::deserialize(deserializer)?;
        let level = Level::from_str(&level)
            .map_err(|_| D::Error::invalid_value(Unexpected::Str(&level), &"valid level"))?;
        Ok(SerializableLevel(level))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum Component {
    Controller,
    Drone { drone_id: DroneId },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogMessage {
    pub component: Component,
    pub target: String,
    pub name: String,
    pub severity: SerializableLevel,
    pub time: DateTime<Utc>,
    pub fields: BTreeMap<String, serde_json::Value>,
}

impl TypedMessage for LogMessage {
    type Response = NoReply;

    fn subject(&self) -> String {
        match &self.component {
            Component::Controller => "logs.controller".into(),
            Component::Drone { drone_id } => format!("logs.drone.{}", drone_id.id()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_component() {
        assert_eq!(
            r#"{"type":"Controller"}"#,
            &serde_json::to_string(&Component::Controller).unwrap()
        );
        assert_eq!(
            r#"{"type":"Drone","drone_id":"foo"}"#,
            &serde_json::to_string(&Component::Drone {
                drone_id: DroneId::new("foo".into())
            })
            .unwrap()
        );
    }
}
