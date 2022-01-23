use clap::Parser;
use futures::StreamExt;
use k8s_openapi::{
    api::core::v1::{Container, Pod, Service, ServicePort, ServiceSpec},
    apimachinery::pkg::apis::meta::v1::OwnerReference,
};
use kube::ResourceExt;
use kube::{
    api::{Api, ListParams, Patch, PatchParams},
    core::ObjectMeta,
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client, Resource,
};
use logging::init_logging;
use serde_json::json;
use spawner_resource::{SessionLivedBackend, SessionLivedBackendStatus, SPAWNER_GROUP};
use std::collections::BTreeMap;
use tokio::time::Duration;

mod logging;

const LABEL_RUN: &str = "run";
const APPLICATION: &str = "spawner-app";
const SIDECAR: &str = "spawner-sidecar";
const TCP: &str = "TCP";
const SIDECAR_PORT: u16 = 9090;

#[derive(Parser, Debug)]
struct Opts {
    #[clap(long, default_value = "default")]
    namespace: String,

    #[clap(long)]
    sidecar: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failure from Kubernetes: {0}")]
    KubernetesFailure(#[source] kube::Error),

    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),
}

struct ControllerContext {
    client: Client,
    namespace: String,
    sidecar: Option<String>,
}

fn run_label(name: &str) -> BTreeMap<String, String> {
    vec![(LABEL_RUN.to_string(), name.to_string())]
        .into_iter()
        .collect()
}

fn owner_reference(meta: &ObjectMeta) -> Result<OwnerReference, Error> {
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

async fn reconcile(
    slbe: SessionLivedBackend,
    ctx: Context<ControllerContext>,
) -> Result<ReconcilerAction, Error> {
    let ControllerContext {
        client,
        namespace,
        sidecar,
    } = ctx.get_ref();

    let name = slbe.name();

    if slbe.status.is_some() {
        tracing::info!(%name, "Ignoring SessionLivedBackend because it already has status metadata.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    }

    let in_port = slbe.spec.http_port;
    let out_port = if sidecar.is_some() {
        SIDECAR_PORT
    } else {
        slbe.spec.http_port
    };

    let pod_api = Api::<Pod>::namespaced(client.clone(), namespace);
    let service_api = Api::<Service>::namespaced(client.clone(), &namespace);
    let slbe_api = Api::<SessionLivedBackend>::namespaced(client.clone(), namespace);

    let owner_reference = owner_reference(&slbe.metadata)?;

    let mut template = slbe.spec.template.clone();
    if let Some(sidecar) = sidecar {
        template.containers.push(Container {
            name: SIDECAR.to_string(),
            image: Some(sidecar.to_string()),
            args: Some(vec![
                format!("http://localhost:{}", in_port),
                format!("{}", SIDECAR_PORT),
            ]),
            ..Container::default()
        });
    }

    
    let pod = pod_api
        .patch(
            &name,
            &PatchParams::apply(SPAWNER_GROUP).force(),
            &Patch::Apply(&Pod {
                metadata: ObjectMeta {
                    name: Some(name.to_string()),
                    labels: Some(run_label(&name)),
                    owner_references: Some(vec![owner_reference.clone()]),
                    ..ObjectMeta::default()
                },
                spec: Some(template),
                ..Pod::default()
            }),
        )
        .await
        .map_err(Error::KubernetesFailure)?;

    let service = service_api
        .patch(
            &name,
            &PatchParams::apply(SPAWNER_GROUP).force(),
            &Patch::Apply(&Service {
                metadata: ObjectMeta {
                    name: Some(name.to_string()),
                    owner_references: Some(vec![owner_reference]),
                    ..ObjectMeta::default()
                },
                spec: Some(ServiceSpec {
                    selector: Some(run_label(&name)),
                    ports: Some(vec![ServicePort {
                        name: Some(APPLICATION.to_string()),
                        protocol: Some(TCP.to_string()),
                        port: out_port as i32,
                        ..ServicePort::default()
                    }]),
                    ..ServiceSpec::default()
                }),
                ..Service::default()
            }),
        )
        .await
        .map_err(Error::KubernetesFailure)?;

    let node_name = if let Some(node_name) = pod
        .spec
        .ok_or(Error::MissingObjectKey("spec (Pod)"))?
        .node_name
    {
        node_name
    } else {
        tracing::info!(%name, "Pod exists but not yet assigned to a node.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    };

    let ip = service
        .spec
        .ok_or(Error::MissingObjectKey("spec (Service)"))?
        .cluster_ip
        .ok_or(Error::MissingObjectKey("spec.clusterIP (Service)"))?;

    let url = format!("http://{}.{}:{}/", name, namespace, out_port);
    let status = SessionLivedBackendStatus {
        ip,
        node_name: node_name.clone(),
        port: out_port,
        url,
    };
    tracing::info!(?status, "status");
    slbe_api
        .patch(
            &name,
            &PatchParams::apply(SPAWNER_GROUP).force(),
            &Patch::Apply(&json!({
                "apiVersion": SessionLivedBackend::api_version(&()).to_string(),
                "kind": SessionLivedBackend::kind(&()).to_string(),
                "metadata": {
                    "labels": {
                        "spawner-group": node_name
                    }
                },
            })),
        )
        .await
        .map_err(Error::KubernetesFailure)?;
    slbe_api
        .patch_status(
            &name,
            &PatchParams::apply(SPAWNER_GROUP).force(),
            &Patch::Apply(&json!({
                "apiVersion": SessionLivedBackend::api_version(&()).to_string(),
                "kind": SessionLivedBackend::kind(&()).to_string(),
                "status": status,
            })),
        )
        .await
        .map_err(Error::KubernetesFailure)?;

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
    let slbes =
        Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    let pods = Api::<Pod>::namespaced(client.clone(), &context.get_ref().namespace);

    Controller::new(slbes, ListParams::default())
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
