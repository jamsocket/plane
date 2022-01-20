use clap::Parser;
use futures::StreamExt;
use futures_util::Stream;
use hyper::{Body, Request, Uri};
use kube::api::DeleteParams;
use kube::ResourceExt;
use kube::{
    api::{Api, ListParams},
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client,
};
use logging::init_logging;
use serde::Deserialize;
use spawner_resource::SessionLivedBackend;
use std::collections::HashSet;
use std::pin::Pin;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

mod logging;

const RETRY_PAUSE_SECS: u64 = 1;
const MAX_RETRIES: u32 = 8;

#[derive(Clone, Deserialize, Debug)]
pub struct MonitorState {
    seconds_since_active: Option<u32>,
    #[allow(unused)]
    live_connections: u32,
}

#[derive(Parser, Debug)]
struct Opts {
    #[clap(long, default_value = "default")]
    namespace: String,

    #[clap(long, default_value = "8080")]
    port: i32,

    #[clap(long)]
    node_name: Option<String>,
}

struct ControllerContext {
    client: Client,
    namespace: String,
    active_listeners: RwLock<HashSet<String>>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid URL field in SessionLivedBackend. {0}")]
    InvalidUrl(String),
}

type MonitorStateStream = Pin<Box<dyn Stream<Item = MonitorState> + Send>>;

pub async fn state_stream(base_uri: &str) -> Result<MonitorStateStream, Error> {
    let client = hyper::Client::new();
    let uri = format!("{}_events", base_uri)
        .parse::<Uri>()
        .map_err(|_| Error::InvalidUrl(base_uri.to_string()))?;

    let stream = async_stream::stream! {
        let mut retry = 0;

        'retry_loop: loop {
            if retry > MAX_RETRIES {
                tracing::warn!(?retry, "Hit max retries.");
                break;
            } else if retry > 0 {
                let duration_secs = RETRY_PAUSE_SECS * (2u64).pow(retry);
                tracing::info!(%duration_secs, "Sleeping.");
                tokio::time::sleep(Duration::from_secs(duration_secs)).await;
            }

            let request = Request::builder()
                .uri(uri.clone())
                .body(Body::empty())
                .expect("Request construction should never fail.");

            let result = match client.request(request).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::warn!(?e, "HTTP error.");
                    retry += 1;
                    continue 'retry_loop;
                }
            };

            let mut stream = result.into_body();

            while let Some(value) = stream.next().await {
                match value {
                    Ok(value) => {
                        if let Ok(value) = serde_json::from_slice(&value) {
                            yield value;
                        } else {
                            tracing::warn!(?value, "Couldn't parse as JSON, restarting connection.");
                            retry += 1;
                            continue 'retry_loop;
                        }
                    },
                    Err(err) => {
                        tracing::warn!(?err, %retry, "HTTP error encountered while streaming messages.");
                        retry += 1;
                        continue 'retry_loop;
                    }
                }
            }
            break; // Stream ran out but didn't fail.
        }
    };

    Ok(Box::pin(stream))
}

pub async fn wait_to_kill_slbe(base_uri: &str, grace_period_seconds: u32) -> Result<(), Error> {
    let base_uri = base_uri.to_string();

    let mut stream = state_stream(&base_uri).await?;
    let mut duration: Option<Duration> = None;

    loop {
        let result = if let Some(duration) = duration {
            match timeout(duration, stream.next()).await {
                Ok(result) => result,
                Err(_) => {
                    tracing::info!("Timed out while waiting for activity.");
                    return Ok(());
                }
            }
        } else {
            stream.next().await
        };

        let result = if let Some(result) = result {
            result
        } else {
            tracing::info!("Reached end of message stream; assuming pod is dead.");
            return Ok(());
        };
        tracing::info!(?result, "Got status message.");

        if let Some(seconds_since_active) = result.seconds_since_active {
            let delta = grace_period_seconds - seconds_since_active.min(grace_period_seconds);
            tracing::info!(delta_seconds = %delta, "Using timeout.");
            duration = Some(Duration::from_secs(delta as u64));
        } else {
            duration = None;
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
        tracing::info!(%name, "Ignoring SessionLivedBackend because it has not yet been scheduled to a node.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    };

    let active_listeners = &ctx.get_ref().active_listeners;
    if active_listeners.read().await.contains(&name) {
        tracing::info!(%name, "Already listening to this SessionLivedBackend, ignoring.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    }

    active_listeners.write().await.insert(name.clone());

    tokio::spawn(async move {
        let client = ctx.get_ref().client.clone();
        let namespace = ctx.get_ref().namespace.clone();

        match wait_to_kill_slbe(&status.url, 30).await {
            Result::Ok(()) => (),
            Result::Err(e) => {
                tracing::error!(?e, "Encountered error testing liveness of SessionLivedBackend, so shutting it down.")
            }
        }

        let slbes = Api::<SessionLivedBackend>::namespaced(client, &namespace);

        match slbes.delete(&name, &DeleteParams::default()).await {
            Result::Ok(_) => {
                tracing::info!(%name, "SessionLivedBackend deleted.");
            }
            Result::Err(error) => {
                tracing::error!(%name, ?error, "Unexpected error deleting SessionLivedBackend.");
            }
        }
    });

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
        active_listeners: RwLock::new(HashSet::default()),
    });

    let query = ListParams {
        label_selector: opts.node_name.map(|name| format!("spawner-group={}", name)),
        ..ListParams::default()
    };

    let slbes =
        Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    Controller::new(slbes, query)
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok(_) => (),
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
