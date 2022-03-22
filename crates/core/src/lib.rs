#![doc = include_str!("../README.md")]

pub use builder::SessionLivedBackendBuilder;
pub use image_pull_policy::ImagePullPolicy;
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
pub use state::SessionLivedBackendState;
use std::{collections::BTreeMap, fmt::Debug};

mod builder;
pub mod event_stream;
mod image_pull_policy;
mod state;

/// A unique string used to avoid namespace clashes with other vendors.
pub const SPAWNER_GROUP: &str = "spawner.dev";

/// Default port on which to expect application containers to listen for HTTP connections on.
const DEFAULT_PORT: u16 = 8080;

/// Port that the sidecar listens on.
const SIDECAR_PORT: u16 = 9090;

/// Name of the label used by a backend's Service to locate the backend's Pod.
const LABEL_RUN: &str = "run";

/// Name used for the sidecar container to differentiate it within a pod.
const SIDECAR: &str = "spawner-sidecar";

/// Name used for the application container to differentiate it within a pod.
const APPLICATION: &str = "spawner-app";

/// Describes a session-lived backend, consisting of a pod template, and optional
/// overrides for the HTTP port and grace period parameters.
///
/// The pod spec should not include the Spawner sidecar container; it is
/// inserted only when the backend is about to be run.
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
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
    /// Describes the container run by the backend.
    pub template: BackendPodSpec,

    /// The period of time (in seconds) after a backend's last connection to wait before
    /// shutting down the backend.
    ///
    /// If another connection is made during the grace period, the countdown is reset.
    pub grace_period_seconds: Option<u32>,

    /// The port of the application container on which we listen for an HTTP connection.
    pub http_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendPodSpec {
    /// Containers to run. If multiple containers are given, the first is assumed to be
    /// the application container.
    containers: Vec<Container>,

    /// A reference to the Kubernetes secret containing image registry access credentials,
    /// if needed by the image registry.
    image_pull_secrets: Option<Vec<LocalObjectReference>>,

    /// Volumes to be attached to the pod associated with this backend.
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

/// Status flags associated with a backend.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionLivedBackendStatus {
    /// Set to `true` by `spawner` once the backing resources (pod and service) have been created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted: Option<bool>,

    /// Set to `true` by `spawner` once the backing pod object exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constructed: Option<bool>,

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

    /// Set to `true` by `spawner` if the pod has failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<bool>,

    /// The name of the Kubernetes cluster node that this backend has been assigned to.
    ///
    /// This is initially None until a backend has been assigned to a node. Once it has,
    /// this never changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,

    /// IP of the service associated with this backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,

    /// Port of the service associated with this backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// In-cluster URL associated with this backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl SessionLivedBackendStatus {
    pub fn patch_state(
        state: SessionLivedBackendState,
        base_status: SessionLivedBackendStatus,
    ) -> SessionLivedBackendStatus {
        match state {
            SessionLivedBackendState::Submitted => SessionLivedBackendStatus {
                submitted: Some(true),
                ..base_status
            },
            SessionLivedBackendState::Constructed => SessionLivedBackendStatus {
                constructed: Some(true),
                ..base_status
            },
            SessionLivedBackendState::Scheduled => SessionLivedBackendStatus {
                scheduled: Some(true),
                ..base_status
            },
            SessionLivedBackendState::Running => SessionLivedBackendStatus {
                running: Some(true),
                ..base_status
            },
            SessionLivedBackendState::Ready => SessionLivedBackendStatus {
                ready: Some(true),
                ..base_status
            },
            SessionLivedBackendState::Swept => SessionLivedBackendStatus {
                swept: Some(true),
                ..base_status
            },
            SessionLivedBackendState::Failed => SessionLivedBackendStatus {
                failed: Some(true),
                ..base_status
            },
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
    pub fn state(&self) -> SessionLivedBackendState {
        if let Some(status) = &self.status {
            if status.swept == Some(true) {
                return SessionLivedBackendState::Swept;
            } else if status.ready == Some(true) {
                return SessionLivedBackendState::Ready;
            } else if status.running == Some(true) {
                return SessionLivedBackendState::Running;
            } else if status.scheduled == Some(true) {
                return SessionLivedBackendState::Scheduled;
            } else if status.constructed == Some(true) {
                return SessionLivedBackendState::Constructed;
            }
        }
        SessionLivedBackendState::Submitted
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

    fn sidecar_container<I>(sidecar_image: &str, http_port: u16, env: I) -> Container
    where
        I: Iterator<Item = (String, String)>,
    {
        let args = vec![
            format!("--serve-port={}", SIDECAR_PORT),
            format!("--upstream-port={}", http_port),
        ];

        let env: Vec<EnvVar> = env
            .filter_map(|(key, val)| {
                let name = key.strip_prefix("SPAWNER_SIDECAR_")?.to_string();
                Some(EnvVar {
                    name,
                    value: Some(val.clone()),
                    ..EnvVar::default()
                })
            })
            .collect();

        let env = if env.is_empty() { None } else { Some(env) };

        Container {
            name: SIDECAR.to_string(),
            image: Some(sidecar_image.to_string()),
            args: Some(args),
            env,
            ..Container::default()
        }
    }

    pub fn pod(
        &self,
        sidecar_image: &str,
        image_pull_secret: &Option<String>,
    ) -> Result<Pod, Error> {
        let name = self.name();
        let http_port = self.spec.http_port.unwrap_or(DEFAULT_PORT);

        let mut template: PodSpec = self.spec.template.clone().into();
        let port_env = EnvVar {
            name: "PORT".to_string(),
            value: Some(http_port.to_string()),
            ..EnvVar::default()
        };

        let first_container = template
            .containers
            .first_mut()
            .ok_or(Error::MissingObjectKey("template.containers[0]"))?;

        match &mut first_container.env {
            Some(env) => {
                env.push(port_env);
            }
            None => first_container.env = Some(vec![port_env]),
        };

        template
            .containers
            .push(Self::sidecar_container(sidecar_image, http_port, std::env::vars()));

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
                    protocol: Some("TCP".to_string()),
                    port: SIDECAR_PORT as i32,
                    ..ServicePort::default()
                }]),
                ..ServiceSpec::default()
            }),
            ..Service::default()
        })
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

    pub fn log_event(&self, state: SessionLivedBackendState) -> Event {
        Event {
            involved_object: self.object_ref(&()),
            action: Some(state.to_string()),
            message: Some(state.message()),
            reason: Some(state.to_string()),
            type_: Some("Normal".to_string()),
            event_time: Some(MicroTime(Utc::now())),
            reporting_component: Some(format!("{}/sessionlivedbackend", SPAWNER_GROUP)),
            reporting_instance: Some(self.name()),
            metadata: ObjectMeta {
                namespace: self.namespace(),
                generate_name: Some(format!("{}-", self.name())),
                ..Default::default()
            },

            ..Default::default()
        }
    }

    pub async fn update_state(
        &self,
        client: Client,
        new_state: SessionLivedBackendState,
        patch_status: SessionLivedBackendStatus,
    ) -> Result<(), Error> {
        let namespace = self
            .namespace()
            .expect("SessionLivedBackend is a namespaced resource, but didn't have a namespace.");
        let slab_api = Api::<SessionLivedBackend>::namespaced(client.clone(), &namespace);
        let event_api = Api::<Event>::namespaced(client.clone(), &namespace);

        let new_status = SessionLivedBackendStatus::patch_state(new_state, patch_status);

        slab_api
            .patch_status(
                &self.name(),
                &PatchParams::default(),
                &Patch::Merge(json!({ "status": new_status })),
            )
            .await
            .map_err(Error::KubernetesFailure)?;

        event_api
            .create(&PostParams::default(), &self.log_event(new_state))
            .await
            .map_err(Error::KubernetesFailure)?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_session_lived_backend_status() {
        let mut backend = SessionLivedBackend {
            spec: SessionLivedBackendSpec::default(),
            metadata: ObjectMeta::default(),
            status: None,
        };
        assert_eq!(SessionLivedBackendState::Submitted, backend.state());

        backend.status = Some(SessionLivedBackendStatus::default());
        assert_eq!(SessionLivedBackendState::Submitted, backend.state());

        backend.status.as_mut().unwrap().constructed = Some(true);
        assert_eq!(SessionLivedBackendState::Constructed, backend.state());

        backend.status.as_mut().unwrap().scheduled = Some(true);
        assert_eq!(SessionLivedBackendState::Scheduled, backend.state());

        backend.status.as_mut().unwrap().running = Some(true);
        assert_eq!(SessionLivedBackendState::Running, backend.state());

        backend.status.as_mut().unwrap().ready = Some(true);
        assert_eq!(SessionLivedBackendState::Ready, backend.state());
    }

    #[test]
    fn test_labels() {
        let backend = SessionLivedBackend {
            spec: SessionLivedBackendSpec::default(),
            metadata: ObjectMeta {
                name: Some("slab1".to_string()),
                namespace: Some("spawner".to_string()),
                labels: Some(
                    vec![("foo".to_string(), "bar".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..ObjectMeta::default()
            },
            status: None,
        };

        let result: Vec<(String, String)> = backend.run_label().into_iter().collect();
        assert_eq!(vec![("run".to_string(), "slab1".to_string())], result);

        let result: Vec<(String, String)> = backend.labels().into_iter().collect();
        assert_eq!(
            vec![
                ("foo".to_string(), "bar".to_string()),
                ("run".to_string(), "slab1".to_string())
            ],
            result
        );
    }

    #[test]
    fn test_metadata_reference() {
        let backend = SessionLivedBackend {
            spec: SessionLivedBackendSpec::default(),
            metadata: ObjectMeta {
                name: Some("slab1".to_string()),
                uid: Some("blah".to_string()),
                ..ObjectMeta::default()
            },
            status: None,
        };

        let reference = backend.metadata_reference().unwrap();

        assert_eq!(
            OwnerReference {
                api_version: "spawner.dev/v1".to_string(),
                block_owner_deletion: None,
                controller: Some(true),
                kind: "SessionLivedBackend".to_string(),
                name: "slab1".to_string(),
                uid: "blah".to_string()
            },
            reference
        );
    }

    #[test]
    fn test_event() {
        let backend = SessionLivedBackend {
            spec: SessionLivedBackendSpec::default(),
            metadata: ObjectMeta {
                namespace: Some("blah".to_string()),
                name: Some("slab1".to_string()),
                ..ObjectMeta::default()
            },
            status: None,
        };

        let event = backend.log_event(SessionLivedBackendState::Running);
        let event_time = event.event_time.clone();

        assert_eq!(
            Event {
                action: Some("Running".to_string()),
                event_time,
                involved_object: backend.object_ref(&()),
                message: Some("Pod was observed running.".to_string()),
                metadata: ObjectMeta {
                    generate_name: Some("slab1-".to_string()),
                    namespace: Some("blah".to_string()),
                    ..ObjectMeta::default()
                },
                reason: Some("Running".to_string()),
                reporting_component: Some("spawner.dev/sessionlivedbackend".to_string()),
                reporting_instance: Some("slab1".to_string()),
                type_: Some("Normal".to_string()),
                ..Event::default()
            },
            event
        );
    }

    #[test]
    fn test_patch_state() {
        assert_eq!(
            SessionLivedBackendStatus {
                submitted: Some(true),
                ..SessionLivedBackendStatus::default()
            },
            SessionLivedBackendStatus::patch_state(
                SessionLivedBackendState::Submitted,
                SessionLivedBackendStatus::default()
            )
        );

        assert_eq!(
            SessionLivedBackendStatus {
                constructed: Some(true),
                ..SessionLivedBackendStatus::default()
            },
            SessionLivedBackendStatus::patch_state(
                SessionLivedBackendState::Constructed,
                SessionLivedBackendStatus::default()
            )
        );

        assert_eq!(
            SessionLivedBackendStatus {
                scheduled: Some(true),
                ip: Some("1.1.1.1".to_string()),
                url: Some("url".to_string()),
                port: Some(8080),
                node_name: Some("node1".to_string()),
                ..SessionLivedBackendStatus::default()
            },
            SessionLivedBackendStatus::patch_state(
                SessionLivedBackendState::Scheduled,
                SessionLivedBackendStatus {
                    ip: Some("1.1.1.1".to_string()),
                    url: Some("url".to_string()),
                    port: Some(8080),
                    node_name: Some("node1".to_string()),
                    ..SessionLivedBackendStatus::default()
                }
            )
        );

        assert_eq!(
            SessionLivedBackendStatus {
                running: Some(true),
                ..SessionLivedBackendStatus::default()
            },
            SessionLivedBackendStatus::patch_state(
                SessionLivedBackendState::Running,
                SessionLivedBackendStatus::default()
            )
        );

        assert_eq!(
            SessionLivedBackendStatus {
                ready: Some(true),
                ..SessionLivedBackendStatus::default()
            },
            SessionLivedBackendStatus::patch_state(
                SessionLivedBackendState::Ready,
                SessionLivedBackendStatus::default()
            )
        );
    }

    #[test]
    fn test_sidecar_env_var() {
        let env = vec![
            ("SPAWNER_SIDECAR_FOO".to_string(), "FOO_VAL".to_string()),
            ("SPAWNER_SIDECAR_BAR".to_string(), "BAR_VAL".to_string()),
        ];

        let sidecar_container = SessionLivedBackend::sidecar_container("sidecar-image", 4040, env.into_iter());

        assert_eq!(
            Container {
                name: "spawner-sidecar".to_string(),
                image: Some("sidecar-image".to_string()),
                args: Some(vec![
                    "--serve-port=9090".to_string(),
                    "--upstream-port=4040".to_string()
                ]),
                env: Some(vec![
                    EnvVar {
                        name: "FOO".to_string(),
                        value: Some("FOO_VAL".to_string()),
                        ..EnvVar::default()
                    },
                    EnvVar {
                        name: "BAR".to_string(),
                        value: Some("BAR_VAL".to_string()),
                        ..EnvVar::default()
                    },
                ]),
                ..Container::default()
            },
            sidecar_container
        );
    }

    #[test]
    fn test_pod_and_service() {
        let backend = SessionLivedBackend {
            spec: SessionLivedBackendSpec {
                template: BackendPodSpec {
                    containers: vec![Container {
                        name: "foo".to_string(),
                        env: Some(vec![EnvVar {
                            name: "FOO".to_string(),
                            value: Some("BAR".to_string()),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    ..BackendPodSpec::default()
                },
                grace_period_seconds: Some(400),
                http_port: Some(4040),
            },
            metadata: ObjectMeta {
                name: Some("slab1".to_string()),
                namespace: Some("ns1".to_string()),
                uid: Some("uid1".to_string()),
                ..ObjectMeta::default()
            },
            status: None,
        };

        assert_eq!(
            Pod {
                metadata: ObjectMeta {
                    name: Some("slab1".to_string()),
                    owner_references: Some(vec![backend.metadata_reference().unwrap()]),
                    labels: Some(
                        vec![("run".to_string(), "slab1".to_string())]
                            .into_iter()
                            .collect()
                    ),
                    ..ObjectMeta::default()
                },
                spec: Some(PodSpec {
                    containers: vec![
                        Container {
                            name: "foo".to_string(),
                            env: Some(vec![
                                EnvVar {
                                    name: "FOO".to_string(),
                                    value: Some("BAR".to_string()),
                                    ..Default::default()
                                },
                                EnvVar {
                                    name: "PORT".to_string(),
                                    value: Some("4040".to_string()),
                                    ..Default::default()
                                },
                            ]),
                            ..Default::default()
                        },
                        Container {
                            name: "spawner-sidecar".to_string(),
                            image: Some("sidecar-image".to_string()),
                            args: Some(vec![
                                "--serve-port=9090".to_string(),
                                "--upstream-port=4040".to_string()
                            ]),
                            ..Container::default()
                        },
                    ],
                    image_pull_secrets: Some(vec![LocalObjectReference {
                        name: Some("my-secret".to_string())
                    }]),
                    ..PodSpec::default()
                }),
                ..Default::default()
            },
            backend
                .pod("sidecar-image", &Some("my-secret".to_string()))
                .unwrap()
        );

        assert_eq!(
            Service {
                metadata: ObjectMeta {
                    name: Some("slab1".to_string()),
                    owner_references: Some(vec![backend.metadata_reference().unwrap()]),
                    labels: Some(
                        vec![("run".to_string(), "slab1".to_string())]
                            .into_iter()
                            .collect()
                    ),
                    ..ObjectMeta::default()
                },
                spec: Some(ServiceSpec {
                    ports: Some(vec![ServicePort {
                        name: Some("spawner-app".to_string()),
                        port: 9090,
                        protocol: Some("TCP".to_string()),
                        ..ServicePort::default()
                    }]),
                    selector: Some(
                        vec![("run".to_string(), "slab1".to_string())]
                            .into_iter()
                            .collect()
                    ),
                    ..ServiceSpec::default()
                }),
                ..Service::default()
            },
            backend.service().unwrap()
        );
    }
}
