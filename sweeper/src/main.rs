use tokio::task::JoinHandle;

use clap::Parser;
use futures::StreamExt;
use hyper::{Body, Request, Uri};
use kube::ResourceExt;
use kube::{
    api::{Api, ListParams},
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client,
};
use logging::init_logging;
use serde::Deserialize;
use spawner_resource::SessionLivedBackend;
use tokio::time::{Duration, timeout};
use futures_util::Stream;

mod logging;

#[derive(Clone, Deserialize, Debug)]
pub struct MonitorState {
    seconds_since_active: Option<u64>,
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
}

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),
}

type BoxStream = Box<dyn Stream<Item = MonitorState>>;

pub async fn state_stream(base_uri: &str) -> impl Stream<Item=MonitorState> {
    let client = hyper::Client::new();
    let uri = format!("{}_events", base_uri).parse::<Uri>().unwrap();
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let result = client.request(request).await.unwrap();
    let stream = result.into_body();

    Box::new(stream.map(|d| serde_json::from_slice(&d.unwrap()).unwrap()))
}

struct StateWatcher {
    grace_period_seconds: u32,
}

impl StateWatcher {
    pub fn new(base_uri: &str, grace_period_seconds: u32, callback: Box<dyn Fn() + Send>) -> Self {
        let base_uri = base_uri.to_string();

        tokio::spawn(async move {
            let mut stream = state_stream(&base_uri).await;
            let mut duration: Option<Duration> = None;

            loop {
                let result = if let Some(duration) = duration {
                    match timeout(duration, stream.next()).await {
                        Ok(result) => result,
                        Err(_) => {
                            callback();
                            return
                        }
                    }
                } else {
                    stream.next().await
                };

                let result = result.unwrap();
                tracing::info!(?result, "Got status message.");
            }
        });

        StateWatcher {
            grace_period_seconds: grace_period_seconds,
        }
    }
}

async fn reconcile(
    slbe: SessionLivedBackend,
    ctx: Context<ControllerContext>,
) -> Result<ReconcilerAction, Error> {
    let name = slbe.name();

    tracing::info!(?name, "Saw slbe.");

    let status = if let Some(status) = slbe.status {
        status
    } else {
        tracing::info!(%name, "Ignoring slbe because it has not yet been scheduled to a node.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    };

    StateWatcher::new(&status.url, 30, Box::new(move || {}));

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
    });

    let query = ListParams {
        label_selector: Some(format!("spawner-group={}", opts.node_name)),
        ..ListParams::default()
    };

    let slbes =
        Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    Controller::new(slbes, query)
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok((object, _)) => (),
                Err(error) => match error {
                    controller::Error::ReconcilerFailed(error, _) => {
                        tracing::error!(%error, "Reconcile failed.")
                    }
                    controller::Error::QueueError(error) => {
                        tracing::error!(%error, "Queue error.")
                    }
                    controller::Error::ObjectNotFound(error) => {
                        tracing::warn!(%error, "Object not found (may have been deleted).")
                    }
                    _ => tracing::error!(%error, "Unhandled reconcile error."),
                },
            }
        })
        .await;
    Ok(())
}
