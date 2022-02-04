use crate::{ImagePullPolicy, SessionLivedBackend, SessionLivedBackendSpec, BackendPodSpec};
use k8s_openapi::api::core::v1::{Container, EnvVar, LocalObjectReference};
use kube::core::ObjectMeta;
use std::collections::HashMap;

pub const DEFAULT_GRACE_SECONDS: u32 = 30;
pub const APPLICATION: &str = "application";
pub const DEFAULT_PREFIX: &str = "spawner-";

pub struct SessionLivedBackendBuilder {
    image: String,
    image_pull_secret: Option<String>,
    env: HashMap<String, String>,
    image_pull_policy: Option<ImagePullPolicy>,
    namespace: Option<String>,
    grace_period_seconds: Option<u32>,
    http_port: Option<u16>,
}

impl SessionLivedBackendBuilder {
    pub fn new(image: &str) -> Self {
        SessionLivedBackendBuilder {
            image: image.to_string(),
            env: HashMap::default(),
            image_pull_policy: None,
            image_pull_secret: None,
            namespace: None,
            grace_period_seconds: Some(DEFAULT_GRACE_SECONDS),
            http_port: None,
        }
    }

    pub fn with_port(self, http_port: Option<u16>) -> Self {
        SessionLivedBackendBuilder { http_port, ..self }
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
        SessionLivedBackendBuilder {
            grace_period_seconds,
            ..self
        }
    }

    pub fn with_image_pull_secret(self, image_pull_secret: Option<String>) -> Self {
        SessionLivedBackendBuilder {
            image_pull_secret,
            ..self
        }
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
            template: BackendPodSpec {
                containers: vec![Container {
                    image: Some(self.image.to_string()),
                    image_pull_policy: self.image_pull_policy.as_ref().map(|d| d.to_string()),
                    env: Some(env),
                    name: APPLICATION.to_string(),
                    ..Default::default()
                }],
                image_pull_secrets: self.image_pull_secret.as_ref().map(|d| {
                    vec![LocalObjectReference {
                        name: Some(d.to_string()),
                    }]
                }),
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
