use std::{collections::HashMap, str::FromStr};

use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec};
use kube::{core::ObjectMeta, CustomResource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SPAWNER_GROUP: &str = "spawner.dev";
pub const DEFAULT_PREFIX: &str = "spawner-";
pub const APPLICATION: &str = "application";
pub const DEFAULT_HTTP_PORT: u16 = 8080;
pub const DEFAULT_GRACE_SECONDS: u32 = 60 * 5;

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "spawner.dev",
    version = "v1",
    kind = "SessionLivedBackend",
    shortname = "slib",
    status = "SessionLivedBackendStatus",
    namespaced
)]
#[serde(rename_all="camelCase")]
pub struct SessionLivedBackendSpec {
    pub template: PodSpec,
    pub grace_period_seconds: Option<u32>,
    pub http_port: u16,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[serde(rename_all="camelCase")]
pub struct SessionLivedBackendStatus {
    pub node_name: String,
    pub ip: String,
    pub port: u16,
    pub url: String,
}

pub struct SessionLivedBackendBuilder {
    image: String,
    env: HashMap<String, String>,
    image_pull_policy: Option<ImagePullPolicy>,
    namespace: Option<String>,
    grace_period_seconds: Option<u32>,
    http_port: u16,
}

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

impl SessionLivedBackendBuilder {
    pub fn new(image: &str) -> Self {
        SessionLivedBackendBuilder {
            image: image.to_string(),
            env: HashMap::default(),
            image_pull_policy: None,
            namespace: None,
            grace_period_seconds: Some(DEFAULT_GRACE_SECONDS),
            http_port: DEFAULT_HTTP_PORT,
        }
    }

    pub fn with_port(self, http_port: u16) -> Self {
        SessionLivedBackendBuilder {
            http_port,
            ..self
        }
    }

    pub fn with_image_pull_policy(self, image_pull_policy: Option<ImagePullPolicy>) -> Self {
        SessionLivedBackendBuilder {
            image_pull_policy,
            ..self
        }
    }

    pub fn with_namespace(self, namespace: Option<String>) -> Self {
        SessionLivedBackendBuilder { namespace, ..self }
    }

    pub fn with_grace_period(self, grace_period_seconds: Option<u32>) -> Self {
        SessionLivedBackendBuilder {grace_period_seconds, ..self}
    }

    pub fn build_spec(&self) -> SessionLivedBackendSpec {
        let env: Vec<EnvVar> = self
            .env
            .iter()
            .map(|(name, value)| EnvVar {
                name: name.to_string(),
                value: Some(value.to_string()),
                ..EnvVar::default()
            })
            .collect();

        SessionLivedBackendSpec {
            http_port: self.http_port,
            grace_period_seconds: self.grace_period_seconds,
            template: PodSpec {
                containers: vec![Container {
                    image: Some(self.image.to_string()),
                    image_pull_policy: self.image_pull_policy.as_ref().map(|d| d.to_string()),
                    env: Some(env),
                    name: APPLICATION.to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        }
    }

    pub fn build_prefixed(&self, prefix: &str) -> SessionLivedBackend {
        SessionLivedBackend {
            metadata: ObjectMeta {
                generate_name: Some(prefix.to_string()),
                ..Default::default()
            },
            spec: self.build_spec(),
            status: None,
        }
    }

    pub fn build_named(&self, name: &str) -> SessionLivedBackend {
        SessionLivedBackend {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: self.namespace.clone(),
                ..Default::default()
            },
            spec: self.build_spec(),
            status: None,
        }
    }

    pub fn build(&self) -> SessionLivedBackend {
        self.build_prefixed(DEFAULT_PREFIX)
    }
}
