use clap::Parser;
use futures::StreamExt;
use k8s_openapi::{
    api::core::v1::{Container, Pod, Service, ServicePort, ServiceSpec},
    apimachinery::pkg::apis::meta::v1::OwnerReference,
};
use kube::{
    api::{Api, ListParams, Patch, PatchParams},
    core::ObjectMeta,
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client, Resource,
};
use logging::init_logging;
use spawner_resource::{SessionLivedBackend, SPAWNER_GROUP};
use std::collections::BTreeMap;
use tokio::time::Duration;

mod logging;

const LABEL_RUN: &str = "run";
const APPLICATION: &str = "spawner-app";
const SIDECAR: &str = "spawner-sidecar";
const TCP: &str = "TCP";

#[derive(Parser, Debug)]
struct Opts {
    #[clap(long, default_value = "default")]
    namespace: String,

    #[clap(long, default_value = "8080")]
    port: i32,

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
    port: i32,
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
        name: meta
            .name
            .as_ref()
            .ok_or(Error::MissingObjectKey(".metadata.name"))?
            .to_string(),
        uid: meta
            .uid
            .as_ref()
            .ok_or(Error::MissingObjectKey(".metadata.uid"))?
            .to_string(),
        ..OwnerReference::default()
    })
}

async fn reconcile(
    g: SessionLivedBackend,
    ctx: Context<ControllerContext>,
) -> Result<ReconcilerAction, Error> {
    let name = g
        .metadata
        .name
        .as_ref()
        .ok_or(Error::MissingObjectKey("metadata.name"))?
        .to_string();
    let pod_api = Api::<Pod>::namespaced(ctx.get_ref().client.clone(), &ctx.get_ref().namespace);
    let service_api =
        Api::<Service>::namespaced(ctx.get_ref().client.clone(), &ctx.get_ref().namespace);

    let owner_reference = owner_reference(&g.metadata)?;

    let mut template = g.spec.template.clone();
    if let Some(sidecar) = &ctx.get_ref().sidecar {
        template.containers.push(Container {
            name: SIDECAR.to_string(),
            image: Some(sidecar.to_string()),
            ..Container::default()
        });
    }

    pod_api
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

    service_api
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
                        port: ctx.get_ref().port,
                        ..ServicePort::default()
                    }]),
                    ..ServiceSpec::default()
                }),
                ..Service::default()
            }),
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
        port: opts.port,
        sidecar: opts.sidecar,
    });
    let slbes =
        Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    Controller::new(slbes, ListParams::default())
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok((object, _)) => tracing::info!(%object.name, "Reconciled."),
                Err(error) => match error {
                    controller::Error::ReconcilerFailed(error, _) => {
                        tracing::error!(%error, "Reconcile failed.")
                    }
                    _ => tracing::error!(%error, "Unhandled reconcile error."),
                },
            }
        })
        .await;
    Ok(())
}
