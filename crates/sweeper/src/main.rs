use clap::Parser;
use dis_spawner::{SessionLivedBackend, SessionLivedBackendState, SessionLivedBackendStatus};
use dis_spawner_tracing::init_logging;
use futures::StreamExt;
use futures_util::Stream;
use hyper::{Body, Request, Uri};
use kube::ResourceExt;
use kube::{
    api::{Api, ListParams},
    runtime::controller::{self, Context, Controller, ReconcilerAction},
    Client,
};
use serde::Deserialize;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

const RETRY_PAUSE_MILIS: u64 = 100;
const MAX_RETRIES: u32 = 100;
const SSE_PREFIX: &[u8] = b"data: ";

#[derive(Clone, Deserialize, Debug)]
pub struct MonitorState {
    #[allow(unused)]
    ready: bool,

    seconds_since_active: Option<u32>,

    #[allow(unused)]
    live_connections: Option<u32>,
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

    #[error("Saw a SessionLivedBackend with no status.")]
    ExpectedStatus,

    #[error("Saw a SessionLivedBackend with a status but no URL.")]
    ExpectedUrl,
}

type MonitorStateStream = Pin<Box<dyn Stream<Item = MonitorState> + Send>>;

pub async fn state_stream(base_uri: &str, name: &str) -> Result<MonitorStateStream, Error> {
    let name = name.to_string();
    let client = hyper::Client::new();
    let uri = format!("{}_events", base_uri)
        .parse::<Uri>()
        .map_err(|_| Error::InvalidUrl(base_uri.to_string()))?;

    let stream = async_stream::stream! {
        let mut retry = 0;

        'retry_loop: loop {
            if retry > MAX_RETRIES {
                tracing::warn!(%uri, %name, %retry, "Hit max retries.");
                break;
            } else if retry > 0 {
                tracing::debug!(%uri, %name, %retry, %RETRY_PAUSE_MILIS, "Sleeping.");
                tokio::time::sleep(Duration::from_millis(RETRY_PAUSE_MILIS)).await;
            }

            let request = Request::builder()
                .uri(uri.clone())
                .body(Body::empty())
                .expect("Request construction should never fail.");

            tracing::debug!(%uri, %name, "Attempting to connect to container.");

            let result = match client.request(request).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::debug!(%uri, %name, ?e, "HTTP error.");
                    retry += 1;
                    continue 'retry_loop;
                }
            };
            retry = 0; // reset retries on successful connection.

            let mut stream = result.into_body();

            while let Some(value) = stream.next().await {
                match value {
                    Ok(value) => {
                        let prefix = &value[..SSE_PREFIX.len()];
                        if prefix != SSE_PREFIX {
                            tracing::warn!(?prefix, "Expected prefix of '{:?}'.", SSE_PREFIX);
                            continue 'retry_loop;
                        }
                        let value = &value[SSE_PREFIX.len()..];
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
        }
    };

    Ok(Box::pin(stream))
}

async fn reconcile(
    slab: Arc<SessionLivedBackend>,
    ctx: Context<ControllerContext>,
) -> Result<ReconcilerAction, Error> {
    let name = slab.as_ref().name();
    tracing::debug!(%name, "Saw SessionLivedBackend.");

    if slab.state() < SessionLivedBackendState::Running {
        tracing::debug!(%name, "Not attempting to connect (not running yet).");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    }

    let url = slab
        .status
        .as_ref()
        .ok_or(Error::ExpectedStatus)?
        .url
        .as_ref()
        .ok_or(Error::ExpectedUrl)?
        .clone();

    let active_listeners = &ctx.get_ref().active_listeners;
    if active_listeners.read().await.contains(&name) {
        tracing::debug!(%name, "Already listening to this SessionLivedBackend, ignoring.");
        return Ok(ReconcilerAction {
            requeue_after: None,
        });
    }

    active_listeners.write().await.insert(name.clone());
    let mut stream = state_stream(&url, &name).await?;

    tokio::spawn(async move {
        let client = ctx.get_ref().client.clone();
        let grace_period_seconds = slab.spec.grace_period_seconds.unwrap_or_default();
        let mut ready = false;
        let mut duration: Option<Duration> = None;

        loop {
            let result = if let Some(duration) = duration {
                match timeout(duration, stream.next()).await {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::debug!(%url, %name, "Timed out without activity.");
                        break;
                    }
                }
            } else {
                stream.next().await
            };

            let result = if let Some(result) = result {
                result
            } else {
                tracing::debug!(%url, %name, "Reached end of message stream; assuming pod is dead.");
                break;
            };

            tracing::debug!(?result, %url, %name, "Got status message.");

            if result.ready & !ready {
                let result = slab
                    .update_state(
                        client.clone(),
                        SessionLivedBackendState::Ready,
                        SessionLivedBackendStatus::default(),
                    )
                    .await;

                if let Err(error) = result {
                    tracing::error!(%name, ?error, "Unexpected error marking SessionLivedBackend as ready.")
                } else {
                    ready = true;
                }
            }

            if let Some(seconds_since_active) = result.seconds_since_active {
                let delta = grace_period_seconds - seconds_since_active.min(grace_period_seconds);
                tracing::debug!(delta_seconds = %delta, %url, %name, "Using timeout.");
                duration = Some(Duration::from_secs(delta as u64));
            } else {
                duration = None;
            }
        }

        loop {
            let result = slab
                .update_state(
                    client.clone(),
                    SessionLivedBackendState::Swept,
                    SessionLivedBackendStatus::default(),
                )
                .await;

            if let Err(error) = result {
                tracing::error!(%name, ?error, "Unexpected error marking SessionLivedBackend as swept.");
                tokio::time::sleep(Duration::from_secs(30)).await;
            } else {
                break;
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

    tracing::debug!(?opts, "Using options");

    let client = Client::try_default().await?;
    let context = Context::new(ControllerContext {
        client: client.clone(),
        namespace: opts.namespace,
        active_listeners: RwLock::new(HashSet::default()),
    });

    let query = ListParams {
        label_selector: opts.node_name.map(|name| format!("spawnerGroup={}", name)),
        ..ListParams::default()
    };

    let slabs =
        Api::<SessionLivedBackend>::namespaced(client.clone(), &context.get_ref().namespace);
    Controller::new(slabs, query)
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
