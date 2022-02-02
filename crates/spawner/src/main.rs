use std::fmt::Debug;

use chrono::Utc;
use clap::Parser;
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::ResourceExt;
use kube::{
    api::{Api, ListParams, Patch, PatchParams},
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client, Resource,
};
use logging::init_logging;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use spawner_resource::{SessionLivedBackend, SPAWNER_GROUP};
use tokio::time::Duration;

const SIDECAR_PORT: u16 = 9090;

mod logging;

#[derive(Parser, Debug)]
struct Opts {
    #[clap(long, default_value = "default")]
    namespace: String,

    #[clap(long)]
    sidecar: String,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failure from Kubernetes: {0}")]
    KubernetesFailure(#[source] kube::Error),

    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),
}

impl From<spawner_resource::Error> for Error {
    fn from(error: spawner_resource::Error) -> Self {
        match error {
            spawner_resource::Error::MissingObjectKey(k) => Error::MissingObjectKey(k),
        }
    }
}

struct ControllerContext {
    client: Client,
    namespace: String,
    sidecar: String,
}

fn get_cluster_ip(service: &Service) -> Option<String> {
    service.spec.as_ref()?.cluster_ip.clone()
}

fn get_node_name(pod: &Pod) -> Option<String> {
    pod.spec.as_ref()?.node_name.clone()
}

async fn patch<T: Resource + Clone + Serialize + DeserializeOwned + Debug, K: Serialize + Debug>(
    api: &Api<T>,
    name: &str,
    patch: K,
) -> Result<T, Error> {
    api.patch(
        name,
        &PatchParams::apply(SPAWNER_GROUP).force(),
        &Patch::Apply(patch),
    )
    .await
    .map_err(Error::KubernetesFailure)
}

async fn patch_status<
    T: Resource + Clone + Serialize + DeserializeOwned + Debug,
    K: Serialize + Debug,
>(
    api: &Api<T>,
    name: &str,
    patch: K,
) -> Result<T, Error> {
    api.patch_status(
        name,
        &PatchParams::apply(SPAWNER_GROUP).force(),
        &Patch::Apply(patch),
    )
    .await
    .map_err(Error::KubernetesFailure)
}

async fn reconcile(
    slab: SessionLivedBackend,
    ctx: Context<ControllerContext>,
) -> Result<ReconcilerAction, Error> {
    let ControllerContext {
        client,
        namespace,
        sidecar,
    } = ctx.get_ref();

    let name = slab.name();

    if slab.is_scheduled() {
        tracing::info!(%name, "Ignoring SessionLivedBackend because it is already scheduled. Sweeper will take over lifecycle management.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    }

    let pod_api = Api::<Pod>::namespaced(client.clone(), namespace);
    let service_api = Api::<Service>::namespaced(client.clone(), &namespace);
    let slab_api = Api::<SessionLivedBackend>::namespaced(client.clone(), namespace);

    let pod = patch(&pod_api, &name, slab.pod(sidecar)?).await?;
    let service = patch(&service_api, &name, slab.service()?).await?;

    let node_name = get_node_name(&pod);
    let ip = get_cluster_ip(&service);
    let url = format!("http://{}.{}:{}/", name, namespace, SIDECAR_PORT);

    if let Some(node_name) = node_name {
        // The pod has been scheduled.
        
        patch_status(
            &slab_api,
            &name,
            json!({
                "apiVersion": SessionLivedBackend::api_version(&()).to_string(),
                "kind": SessionLivedBackend::kind(&()).to_string(),
                "status": {
                    "nodeName": node_name,
                    "ip": ip,
                    "url": url,
                    "port": SIDECAR_PORT,
                    "scheduledTime": Time(Utc::now()),
                },
            }),
        )
        .await?;

        patch(
            &slab_api,
            &name,
            json!({
                "apiVersion": SessionLivedBackend::api_version(&()).to_string(),
                "kind": SessionLivedBackend::kind(&()).to_string(),
                "metadata": {
                    "labels": {
                        "spawner-group": node_name
                    }
                },
            }),
        )
        .await?;
    } else {
        // The pod exists but has not been scheduled.

        patch_status(
            &slab_api,
            &name,
            json!({
                "apiVersion": SessionLivedBackend::api_version(&()).to_string(),
                "kind": SessionLivedBackend::kind(&()).to_string(),
                "status": {
                    "startedTime": Time(Utc::now()),
                },
            }),
        )
        .await?;
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

    tracing::info!(?opts, "Using options");

    let client = Client::try_default().await?;
    let context = Context::new(ControllerContext {
        client: client.clone(),
        namespace: opts.namespace,
        sidecar: opts.sidecar,
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
                    controller::Error::ObjectNotFound(error) => {
                        tracing::warn!(%error, "Object not found (may have been deleted).")
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
