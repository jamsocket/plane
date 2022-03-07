#![doc = include_str!("../README.md")]

pub use builder::SessionLivedBackendBuilder;
use k8s_openapi::{
    api::core::v1::{
        Container, EnvVar, Event, LocalObjectReference, Pod, PodSpec, Service, ServicePort,
        ServiceSpec, Volume,
    },
    apimachinery::pkg::apis::meta::v1::{MicroTime, OwnerReference},
    chrono::Utc,
};
use kube::{
    api::{Patch, PatchParams, PostParams},
    Api, Client, CustomResource, Resource,
};
use kube::{core::ObjectMeta, ResourceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fmt::{Debug, Display},
    str::FromStr,
};

mod builder;
pub mod event_stream;

pub const SPAWNER_GROUP: &str = "spawner.dev";
const DEFAULT_PORT: u16 = 8080;
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
    pub template: BackendPodSpec,
    pub grace_period_seconds: Option<u32>,
    pub http_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendPodSpec {
    containers: Vec<Container>,
    image_pull_secrets: Option<Vec<LocalObjectReference>>,
    volumes: Option<Vec<Volume>>,
}

impl Into<PodSpec> for BackendPodSpec {
    fn into(self) -> PodSpec {
        PodSpec {
            containers: self.containers,
            image_pull_secrets: self.image_pull_secrets,
            volumes: self.volumes,
            ..Default::default()
        }
    }
}

#[derive(Debug)]
pub enum SessionLivedBackendEvent {
    /// The `SessionLivedBackend` object exists.
    Submitted,

    /// The pod and service backing a `SessionLivedBackend` exist.
    Constructed,

    /// The pod that backs a `SessionLivedBackend` has been assigned to a node.
    Scheduled,

    /// The pod that backs a `SessionLivedBackend` is running.
    Running,

    /// The pod that backs a `SessionLivedBackend` is accepting new connections.
    Ready,

    /// The `SessionLivedBackend` has been marked as swept, meaning that it can be deleted.
    Swept,
}

impl Default for SessionLivedBackendEvent {
    fn default() -> Self {
        SessionLivedBackendEvent::Submitted
    }
}

impl Display for SessionLivedBackendEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self, f)
    }
}

impl SessionLivedBackendEvent {
    pub fn message(&self) -> String {
        match self {
            SessionLivedBackendEvent::Submitted => {
                "SessionLivedBackend object created.".to_string()
            }
            SessionLivedBackendEvent::Constructed => {
                "Backing resources created by Spawner.".to_string()
            }
            SessionLivedBackendEvent::Scheduled => {
                "Backing pod was scheduled by Kubernetes.".to_string()
            }
            SessionLivedBackendEvent::Running => "Pod was observed running.".to_string(),
            SessionLivedBackendEvent::Ready => {
                "Pod was observed listening on TCP port.".to_string()
            }
            SessionLivedBackendEvent::Swept => {
                "SessionLivedBackend was found idle and swept.".to_string()
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionLivedBackendStatus {
    /// Set to `true` by `spawner` once the backing resources (pod and service) have been created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted: Option<bool>,

    /// Set to `true` by `spawner` once the backing pod has been assigned to a node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<bool>,

    /// Set to `true` by `spawner` once the backing pod is observed in the `Running` state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,

    /// Set to `true` by `sweeper` once the `sidecar` proxy issues an event with `ready`
    /// set to `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,

    /// Set to `true` by `sweeper` once the pod is observed to be inactive (per `sidecar`
    /// events) for at least its grace period. This marks the `SessionLivedBackend` for
    /// deletion by `spawner`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swept: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    #[serde(skip_serializing_if = "Option::is_none")]
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

    #[error("Failure from Kubernetes: {0}")]
    KubernetesFailure(#[source] kube::Error),
}

impl SessionLivedBackend {
    pub fn is_submitted(&self) -> bool {
        self.status
            .as_ref()
            .map(|d| d.submitted.unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn is_scheduled(&self) -> bool {
        self.status
            .as_ref()
            .map(|d| d.scheduled.unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.status
            .as_ref()
            .map(|d| d.running.unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn is_ready(&self) -> bool {
        self.status
            .as_ref()
            .map(|d| d.ready.unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn is_swept(&self) -> bool {
        self.status
            .as_ref()
            .map(|d| d.swept.unwrap_or_default())
            .unwrap_or_default()
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

    fn labels(&self) -> BTreeMap<String, String> {
        let mut labels = self.metadata.labels.clone().unwrap_or_default();
        labels.append(&mut self.run_label());
        labels
    }

    pub fn pod(
        &self,
        sidecar_image: &str,
        image_pull_secret: &Option<String>,
    ) -> Result<Pod, Error> {
        let name = self.name();
        let http_port = self.spec.http_port.unwrap_or(DEFAULT_PORT);
        let args = vec![
            format!("--serve-port={}", SIDECAR_PORT),
            format!("--upstream-port={}", http_port),
        ];

        let mut template: PodSpec = self.spec.template.clone().into();
        template.containers.insert(
            0,
            Container {
                name: SIDECAR.to_string(),
                image: Some(sidecar_image.to_string()),
                args: Some(args),
                ..Container::default()
            },
        );

        let port_env = EnvVar {
            name: "PORT".to_string(),
            value: Some(http_port.to_string()),
            ..EnvVar::default()
        };
        template.containers[1].env = match &template.containers[1].env {
            Some(env) => {
                let mut env = env.clone();
                env.push(port_env);
                Some(env)
            }
            None => Some(vec![port_env]),
        };

        if let Some(image_pull_secret) = image_pull_secret {
            let secret_ref = LocalObjectReference {
                name: Some(image_pull_secret.to_string()),
            };

            template.image_pull_secrets = match template.image_pull_secrets {
                Some(mut v) => {
                    v.push(secret_ref);
                    Some(v)
                }
                None => Some(vec![secret_ref]),
            }
        }

        Ok(Pod {
            metadata: ObjectMeta {
                name: Some(name),
                labels: Some(self.labels()),
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
                labels: Some(self.labels()),
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

    pub async fn log_event(
        &self,
        client: Client,
        new_state: SessionLivedBackendEvent,
    ) -> Result<(), Error> {
        tracing::info!(name=%self.name(), %new_state, "SessionLivedBackend state change.");
        let namespace = self
            .namespace()
            .expect("SessionLivedBackend is a namespaced resource, but didn't have a namespace.");
        let event_api = Api::<Event>::namespaced(client.clone(), &namespace);

        event_api
            .create(
                &PostParams::default(),
                &Event {
                    involved_object: self.object_ref(&()),
                    action: Some(new_state.to_string()),
                    message: Some(new_state.message()),
                    reason: Some(new_state.to_string()),
                    type_: Some("Normal".to_string()),
                    event_time: Some(MicroTime(Utc::now())),
                    reporting_component: Some("spawner.dev/sessionlivedbackend".to_string()),
                    reporting_instance: Some(self.name()),
                    metadata: ObjectMeta {
                        namespace: self.namespace(),
                        generate_name: Some(format!("{}-", self.name())),
                        ..Default::default()
                    },

                    ..Default::default()
                },
            )
            .await
            .map_err(Error::KubernetesFailure)?;

        Ok(())
    }

    pub async fn set_spawner_group(
        &self,
        client: Client,
        spawner_group: &str,
    ) -> Result<(), Error> {
        let namespace = self
            .namespace()
            .expect("SessionLivedBackend is a namespaced resource, but didn't have a namespace.");
        let slab_api = Api::<SessionLivedBackend>::namespaced(client.clone(), &namespace);

        slab_api
            .patch(
                &self.name(),
                &PatchParams::default(),
                &Patch::Merge(json!({
                    "metadata": {
                        "labels": {
                            "spawnerGroup": spawner_group
                        }
                    }
                })),
            )
            .await
            .map_err(Error::KubernetesFailure)?;

        Ok(())
    }

    pub async fn update_status(
        &self,
        client: Client,
        new_status: SessionLivedBackendStatus,
    ) -> Result<(), Error> {
        let namespace = self
            .namespace()
            .expect("SessionLivedBackend is a namespaced resource, but didn't have a namespace.");
        let slab_api = Api::<SessionLivedBackend>::namespaced(client.clone(), &namespace);

        slab_api
            .patch_status(
                &self.name(),
                &PatchParams::default(),
                &Patch::Merge(json!({ "status": new_status })),
            )
            .await
            .map_err(Error::KubernetesFailure)?;

        Ok(())
    }
}
