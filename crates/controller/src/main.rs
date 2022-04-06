use clap::Parser;
use dis_spawner::{SessionLivedBackend, SessionLivedBackendState, SessionLivedBackendStatus};
use dis_spawner_tracing::init_logging;
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::{DeleteParams, PostParams};
use kube::ResourceExt;
use kube::{
    api::{Api, ListParams},
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client,
};
use std::fmt::Debug;
use std::sync::Arc;
use tokio::time::Duration;

const SIDECAR_PORT: u16 = 9090;

#[derive(Parser, Debug)]
struct Opts {
    #[clap(long, default_value = "spawner")]
    namespace: String,

    #[clap(
        long,
        default_value = "ghcr.io/drifting-in-space/spawner-sidecar:latest"
    )]
    sidecar: String,

    /// The name of a Kubernetes secret (type kubernetes.io/dockerconfigjson) for loading the container image.
    ///
    /// See: https://kubernetes.io/docs/tasks/configure-pod-container/pull-image-private-registry/
    #[clap(long)]
    image_pull_secret: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failure from Kubernetes: {0}")]
    KubernetesFailure(#[source] kube::Error),

    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),

    #[error("Serialization error")]
    SerializationError,
}

impl From<dis_spawner::Error> for Error {
    fn from(error: dis_spawner::Error) -> Self {
        match error {
            dis_spawner::Error::MissingObjectKey(k) => Error::MissingObjectKey(k),
            dis_spawner::Error::KubernetesFailure(e) => Error::KubernetesFailure(e),
        }
    }
}

struct ControllerContext {
    client: Client,
    namespace: String,
    sidecar: String,
    image_pull_secret: Option<String>,
}

fn get_cluster_ip(service: &Service) -> Option<String> {
    service.spec.as_ref()?.cluster_ip.clone()
}

fn get_node_name(pod: &Pod) -> Option<String> {
    pod.spec.as_ref()?.node_name.clone()
}

fn get_pod_phase(pod: &Pod) -> Option<String> {
    pod.status.as_ref()?.phase.clone()
}

#[derive(Debug)]
#[allow(unused)]
enum ContainerState {
    Running,
    Terminated,
    Failed,
}

fn get_application_state(pod: &Pod) -> Option<ContainerState> {
    for status in pod.status.as_ref()?.container_statuses.as_ref()? {
        if status.name == "application" {
            let state = status.state.as_ref()?;

            tracing::debug!(?state, "saw container in state");

            if let Some(terminated) = state.terminated.as_ref() {
                if terminated.reason.as_deref() == Some("Error") {
                    return Some(ContainerState::Failed);
                }
            }
        }
    }

    None
}

async fn reconcile(
    slab: Arc<SessionLivedBackend>,
    ctx: Context<ControllerContext>,
) -> Result<ReconcilerAction, Error> {
    let ControllerContext {
        client,
        namespace,
        sidecar,
        image_pull_secret,
    } = ctx.get_ref();

    let name = slab.name();
    let pod_api = Api::<Pod>::namespaced(client.clone(), namespace);
    let service_api = Api::<Service>::namespaced(client.clone(), &namespace);
    let slab_api = Api::<SessionLivedBackend>::namespaced(client.clone(), &namespace);

    match slab.state() {
        SessionLivedBackendState::Submitted => {
            pod_api
                .create(
                    &PostParams::default(),
                    &slab.pod(sidecar, image_pull_secret)?,
                )
                .await
                .map_err(Error::KubernetesFailure)?;

            // Construct service to back session-lived backend.
            service_api
                .create(&PostParams::default(), &slab.service()?)
                .await
                .map_err(Error::KubernetesFailure)?;

            slab.update_state(
                client.clone(),
                SessionLivedBackendState::Constructed,
                SessionLivedBackendStatus::default(),
            )
            .await?;
        }
        SessionLivedBackendState::Constructed => {
            let pod = pod_api.get(&name).await.map_err(Error::KubernetesFailure)?;
            let service = service_api
                .get(&name)
                .await
                .map_err(Error::KubernetesFailure)?;

            if let Some(node_name) = get_node_name(&pod) {
                let ip = get_cluster_ip(&service);
                let url = format!("http://{}.{}:{}/", name, namespace, SIDECAR_PORT);

                slab.set_spawner_group(client.clone(), &node_name).await?;

                let status = SessionLivedBackendStatus {
                    node_name: Some(node_name),
                    ip,
                    url: Some(url),
                    ..Default::default()
                };
                slab.update_state(client.clone(), SessionLivedBackendState::Scheduled, status)
                    .await?;
            }
        }
        SessionLivedBackendState::Scheduled => {
            let pod = pod_api.get(&name).await.map_err(Error::KubernetesFailure)?;
            if let Some(phase) = get_pod_phase(&pod) {
                tracing::debug!(%phase, %name, "Saw pod in phase.");

                if phase == "Running" {
                    slab.update_state(
                        client.clone(),
                        SessionLivedBackendState::Running,
                        SessionLivedBackendStatus::default(),
                    )
                    .await?;
                }
            }
        }
        SessionLivedBackendState::Running => {
            let pod = pod_api.get(&name).await.map_err(Error::KubernetesFailure)?;
            let state = get_application_state(&pod);

            match state {
                Some(ContainerState::Failed) => {
                    slab.update_state(
                        client.clone(),
                        SessionLivedBackendState::Failed,
                        SessionLivedBackendStatus::default(),
                    )
                    .await?;
                }
                _ => (),
            }

            // If there is no error, Sweeper makes the next state change (to swept.)
        }
        SessionLivedBackendState::Ready => {
            // Sweeper makes the next state change.
        }
        SessionLivedBackendState::Failed => {
            // TODO: sweep after a time interval.
        }
        SessionLivedBackendState::Swept => {
            match slab_api.delete(&name, &DeleteParams::default()).await {
                Result::Ok(_) => {
                    tracing::debug!(%name, "SessionLivedBackend deleted.");
                }
                Result::Err(error) => {
                    tracing::error!(%name, ?error, "Unexpected error deleting SessionLivedBackend.");
                    return Err(Error::KubernetesFailure(error));
                }
            }
        }
    }

    Ok(ReconcilerAction {
        requeue_after: None,
    })
}

fn error_policy(_error: &Error, _ctx: Context<ControllerContext>) -> ReconcilerAction {
    ReconcilerAction {
        requeue_after: Some(Duration::from_secs(10)),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    let opts = Opts::parse();

    tracing::debug!(?opts, "Using options");

    let client = Client::try_default().await?;
    let context = Context::new(ControllerContext {
        client: client.clone(),
        namespace: opts.namespace,
        sidecar: opts.sidecar,
        image_pull_secret: opts.image_pull_secret,
    });
    let slabs =
        Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    let pods = Api::<Pod>::namespaced(client.clone(), &context.get_ref().namespace);

    Controller::new(slabs, ListParams::default())
        .owns(pods, ListParams::default())
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok(_) => (),
                Err(error) => match error {
                    controller::Error::ReconcilerFailed(error, _) => {
                        tracing::error!(%error, "Reconcile failed.")
                    }
                    controller::Error::ObjectNotFound(_error) => {
                        // This happens when we've deleted the SessionLivedBackend.
                    }
                    controller::Error::QueueError(error) => {
                        tracing::error!(%error, "Queue error.")
                    }
                    _ => tracing::error!(%error, "Unhandled reconcile error."),
                },
            }
        })
        .await;
    Ok(())
}
