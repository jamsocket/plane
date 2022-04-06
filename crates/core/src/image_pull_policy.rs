use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum ImagePullPolicy {
    Always,
    Never,
    IfNotPresent,
}

impl ToString for ImagePullPolicy {
    fn to_string(&self) -> String {
        match self {
            ImagePullPolicy::Always => "Always",
            ImagePullPolicy::Never => "Never",
            ImagePullPolicy::IfNotPresent => "IfNotPresent",
        }
        .to_string()
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Unknown ImagePullPolicy: {0}.")]
pub struct BadPolicyName(String);

impl FromStr for ImagePullPolicy {
    type Err = BadPolicyName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Always" => Ok(ImagePullPolicy::Always),
            "Never" => Ok(ImagePullPolicy::Never),
            "IfNotPresent" => Ok(ImagePullPolicy::IfNotPresent),
            _ => Err(BadPolicyName(s.to_string())),
        }
    }
}
