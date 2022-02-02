use k8s_openapi::{
    api::core::v1::{Container, Pod, PodSpec, Service, ServiceSpec, ServicePort},
    apimachinery::pkg::apis::meta::v1::{OwnerReference, Time},
};
use kube::{CustomResource, core::ObjectMeta, ResourceExt};
use kube::Resource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};
pub use builder::SessionLivedBackendBuilder;

mod builder;

pub const SPAWNER_GROUP: &str = "spawner.dev";
const LABEL_RUN: &str = "run";
const SIDECAR_PORT: u16 = 9090;
const SIDECAR: &str = "spawner-sidecar";
const APPLICATION: &str = "spawner-app";
const TCP: &str = "TCP";

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "spawner.dev",
    version = "v1",
    kind = "SessionLivedBackend",
    shortname = "slab",
    status = "SessionLivedBackendStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct SessionLivedBackendSpec {
    pub template: PodSpec,
    pub grace_period_seconds: Option<u32>,
    pub http_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionLivedBackendStatus {
    /// When the backend was started. Set by Spawner.
    pub started_time: Option<Time>,

    /// When the backend was scheduled. Set by Spawner.
    pub scheduled_time: Option<Time>,

    /// When the backend started running the application container. Set by Sweeper.
    pub initializing_time: Option<Time>,

    /// When the backend started listening for connections. Set by Sweeper.
    pub ready_time: Option<Time>,

    pub node_name: Option<String>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub url: Option<String>,
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

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),
}

impl SessionLivedBackend {
    /// Return true if the SessionLivedBackend has been assigned to a node.
    /// 
    /// This is important to the lifecycle handoff between Spawner and Sweeper.
    /// Before a node is scheduled, Spawner manages its status; after a node is
    /// scheduled, Sweeper manages its status.
    pub fn is_scheduled(&self) -> bool {
        if let Some(status) = &self.status {
            status.scheduled_time.is_some()
        } else {
            false
        }        
    }

    fn metadata_reference(&self) -> Result<OwnerReference, Error> {
        let meta = &self.metadata;

        Ok(OwnerReference {
            api_version: SessionLivedBackend::api_version(&()).to_string(),
            kind: SessionLivedBackend::kind(&()).to_string(),
            controller: Some(true),
            name: meta
                .name
                .as_ref()
                .ok_or(Error::MissingObjectKey("metadata.name"))?
                .to_string(),
            uid: meta
                .uid
                .as_ref()
                .ok_or(Error::MissingObjectKey("metadata.uid"))?
                .to_string(),
            ..OwnerReference::default()
        })
    }

    fn run_label(&self) -> BTreeMap<String, String> {
        vec![(LABEL_RUN.to_string(), self.name())]
            .into_iter()
            .collect()
    }

    pub fn pod(&self, sidecar_image: &str) -> Result<Pod, Error> {
        let name = self.name();
        let mut args = vec![format!("--serve-port={}", SIDECAR_PORT)];
        if let Some(port) = self.spec.http_port {
            args.push(format!("--upstream-port={}", port))
        };

        let mut template = self.spec.template.clone();
        template.containers.push(Container {
            name: SIDECAR.to_string(),
            image: Some(sidecar_image.to_string()),
            args: Some(args),
            ..Container::default()
        });

        Ok(Pod {
            metadata: ObjectMeta {
                name: Some(name),
                labels: Some(self.run_label()),
                owner_references: Some(vec![self.metadata_reference()?]),
                ..ObjectMeta::default()
            },
            spec: Some(template),
            ..Pod::default()
        })
    }

    pub fn service(&self) -> Result<Service, Error> {
        let name = self.name();

        Ok(Service {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                owner_references: Some(vec![self.metadata_reference()?]),
                ..ObjectMeta::default()
            },
            spec: Some(ServiceSpec {
                selector: Some(self.run_label()),
                ports: Some(vec![ServicePort {
                    name: Some(APPLICATION.to_string()),
                    protocol: Some(TCP.to_string()),
                    port: SIDECAR_PORT as i32,
                    ..ServicePort::default()
                }]),
                ..ServiceSpec::default()
            }),
            ..Service::default()
        })
    }
}
