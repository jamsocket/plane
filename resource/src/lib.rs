use std::{collections::HashMap, str::FromStr};

use k8s_openapi::api::core::v1::{PodSpec, Container, EnvVar};
use kube::{CustomResource, core::ObjectMeta};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SPAWNER_GROUP: &str = "spawner.dev";
pub const DEFAULT_PREFIX: &str = "spawner-";

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "spawner.dev",
    version = "v1",
    kind = "SessionLivedBackend",
    shortname = "slbe",
    namespaced
)]

pub struct SessionLivedBackendSpec {
    pub template: PodSpec,
}

pub struct SessionLivedBackendBuilder {
    image: String,
    env: HashMap<String, String>,
    image_pull_policy: Option<ImagePullPolicy>,
    namespace: Option<String>,
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
        }.to_string()
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
            _ => Err(BadPolicyName(s.to_string()))
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
        }
    }

    pub fn with_image_pull_policy(self, image_pull_policy: Option<ImagePullPolicy>) -> Self {
        SessionLivedBackendBuilder {
            image_pull_policy,
            ..self
        }
    }

    pub fn with_namespace(self, namespace: Option<String>) -> Self {
        SessionLivedBackendBuilder {
            namespace,
            ..self
        }
    }

    pub fn build_spec(&self) -> SessionLivedBackendSpec {
        let env: Vec<EnvVar> = self.env.iter().map(|(name, value)| EnvVar {
            name: name.to_string(),
            value: Some(value.to_string()),
            ..EnvVar::default()
        }).collect();

        SessionLivedBackendSpec {
            template: PodSpec {
                containers: vec![
                    Container {
                        image: Some(self.image.to_string()),
                        image_pull_policy: self.image_pull_policy.as_ref().map(|d| d.to_string()),
                        env: Some(env),
                        ..Default::default()
                    }
                ],
                ..Default::default()
            }
        }
    }

    pub fn build_prefixed(&self, prefix: &str) -> SessionLivedBackend {
        SessionLivedBackend {
            metadata: ObjectMeta {
                generate_name: Some(prefix.to_string()),
                ..Default::default()
            },
            spec: self.build_spec(),
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
        }
    }

    pub fn build(&self) -> SessionLivedBackend {
        self.build_prefixed(DEFAULT_PREFIX)
    }
}
