pub use builder::SessionLivedBackendBuilder;
use k8s_openapi::{
    api::core::v1::{
        Container, Event, LocalObjectReference, Pod, PodSpec, Service, ServicePort, ServiceSpec,
        Volume,
    },
    apimachinery::pkg::apis::meta::v1::{MicroTime, OwnerReference},
    chrono::Utc,
};
use kube::{
    api::{Patch, PatchParams, PostParams},
    Api, Client, Resource, CustomResource,
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Copy)]
pub enum SessionLivedBackendState {
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
}

impl Default for SessionLivedBackendState {
    fn default() -> Self {
        SessionLivedBackendState::Submitted
    }
}

impl Display for SessionLivedBackendState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self, f)
    }
}

impl SessionLivedBackendState {
    pub fn message(&self) -> String {
        match self {
            SessionLivedBackendState::Submitted => {
                "SessionLivedBackend object created.".to_string()
            }
            SessionLivedBackendState::Constructed => {
                "Backing resources created by Spawner.".to_string()
            }
            SessionLivedBackendState::Scheduled => {
                "Backing pod was scheduled by Kubernetes.".to_string()
            }
            SessionLivedBackendState::Running => "Pod was observed running.".to_string(),
            SessionLivedBackendState::Ready => {
                "Pod was observed listening on TCP port.".to_string()
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionLivedBackendStatus {
    pub state: SessionLivedBackendState,

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

    #[error("Failure from Kubernetes: {0}")]
    KubernetesFailure(#[source] kube::Error),
}

impl SessionLivedBackend {
    /// Return true if the SessionLivedBackend has been assigned to a node.
    ///
    /// This is important to the lifecycle handoff between Spawner and Sweeper.
    /// Before a node is scheduled, Spawner manages its status; after a node is
    /// scheduled, Sweeper manages its status.
    pub fn is_scheduled(&self) -> bool {
        match self.state() {
            SessionLivedBackendState::Submitted => false,
            SessionLivedBackendState::Constructed => false,
            SessionLivedBackendState::Scheduled => true,
            SessionLivedBackendState::Running => true,
            SessionLivedBackendState::Ready => true,
        }
    }

    pub fn is_ready(&self) -> bool {
        match self.state() {
            SessionLivedBackendState::Submitted => false,
            SessionLivedBackendState::Constructed => false,
            SessionLivedBackendState::Scheduled => false,
            SessionLivedBackendState::Running => false,
            SessionLivedBackendState::Ready => true,
        }
    }

    pub fn state(&self) -> SessionLivedBackendState {
        if let Some(status) = &self.status {
            status.state
        } else {
            SessionLivedBackendState::Submitted
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

        let mut template: PodSpec = self.spec.template.clone().into();
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

    async fn log_state_change(
        &self,
        events_api: &Api<Event>,
        new_state: SessionLivedBackendState,
    ) -> Result<(), Error> {
        tracing::info!(name=%self.name(), %new_state, "SessionLivedBackend state change.");

        events_api
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

    pub async fn update_status(
        &self,
        client: Client,
        new_status: SessionLivedBackendStatus,
    ) -> Result<(), Error> {
        let namespace = self
            .namespace()
            .expect("SessionLivedBackend is a namespaced resource, but didn't have a namespace.");
        let slab_api = Api::<SessionLivedBackend>::namespaced(client.clone(), &namespace);
        let event_api = Api::<Event>::namespaced(client.clone(), &namespace);

        slab_api
            .patch_status(
                &self.name(),
                &PatchParams::default(),
                &Patch::Merge(json!({ "status": new_status })),
            )
            .await
            .map_err(Error::KubernetesFailure)?;

        self.log_state_change(&event_api, new_status.state).await?;

        Ok(())
    }
}
