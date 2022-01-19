use clap::Parser;
use futures::StreamExt;
use kube::Resource;
use kube::{
    api::{Api, ListParams},
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client,
};
use logging::init_logging;
use serde::Deserialize;
use spawner_resource::{SessionLivedBackend, SessionLivedBackendBuilder};
use tokio::time::Duration;
use kube::ResourceExt;

mod logging;

#[derive(Clone, Deserialize)]
pub struct MonitorState {
    last_active: u64,
    live_connections: u32,
}

#[derive(Parser, Debug)]
struct Opts {
    #[clap(long, default_value = "default")]
    namespace: String,

    #[clap(long, default_value = "8080")]
    port: i32,

    #[clap(long, default_value = "minikube")]
    node_name: String,
}

struct ControllerContext {
    client: Client,
    namespace: String,
    port: i32,
}

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),
}

async fn reconcile(slbe: SessionLivedBackend, ctx: Context<ControllerContext>) -> Result<ReconcilerAction, Error> {
    let name = slbe.name();

    tracing::info!(?name, "Saw slbe.");

    let status = if let Some(status) = slbe.status {
        status
    } else {
        tracing::info!(%name, "Ignoring slbe because it has not yet been scheduled to a node.");
        return Ok(ReconcilerAction {requeue_after: None})
    };

    tracing::info!(?status, "Status");

    // let client = hyper::Client::new();
    // let result = client.get("http://{}:{}/_events");

    // let boyd = result.await.unwrap().body();

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
    });

    let query = ListParams {
        label_selector: Some(format!("spawner-group={}", opts.node_name)),
        ..ListParams::default()
    };

    let slbes = Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    Controller::new(slbes, query)
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok((object, _)) => tracing::info!(%object.name, "Reconciled."),
                Err(error) => match error {
                    controller::Error::ReconcilerFailed(error, _) => {
                        tracing::error!(%error, "Reconcile failed.")
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
