use anyhow::Result;
use chrono::{DateTime, Utc};
use core::str::FromStr;
use serde::de::{Error, Unexpected};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::Level;

use crate::nats::{NoReply, Subject};

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

#[derive(Debug, Serialize, Deserialize)]
pub struct LogMessage {
    pub target: String,
    pub name: String,
    pub severity: SerializableLevel,
    pub time: DateTime<Utc>,
    pub fields: BTreeMap<String, serde_json::Value>,
}

impl LogMessage {
    #[must_use]
    pub fn subject(subject_name: &str) -> Subject<LogMessage, NoReply> {
        Subject::new(subject_name.to_string())
    }
}
