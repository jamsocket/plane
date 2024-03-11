use super::{backend_manager::BackendManager, docker::PlaneDocker, state_store::StateStore};
use crate::{
    database::backend::BackendMetricsMessage,
    names::BackendName,
    protocol::{BackendAction, BackendEventId, BackendStateMessage},
    typed_socket::TypedSocketSender,
    types::BackendState,
    util::GuardHandle,
};
use anyhow::Result;
use chrono::{Duration, Utc};
use dashmap::DashMap;
use futures_util::StreamExt;
use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

/// Clean up containers and images every minute.
const CLEANUP_INTERVAL_SECS: i64 = 60;

pub struct Executor {
    docker: PlaneDocker,
    state_store: Arc<Mutex<StateStore>>,
    backends: Arc<DashMap<BackendName, Arc<BackendManager>>>,
    ip: IpAddr,
    _backend_event_listener: GuardHandle,
    _cleanup_handle: GuardHandle,
}

async fn cleanup_loop(
    docker: PlaneDocker,
    min_age: Duration,
    interval: Duration,
    auto_prune: bool,
) {
    loop {
        tokio::time::sleep(
            interval
                .to_std()
                .expect("Expected interval to convert to std."),
        )
        .await;

        let since = Utc::now() - min_age;

        if let Err(e) = docker.prune(since, auto_prune).await {
            tracing::error!(?e, "Error pruning Docker containers and images.");
        }
    }
}

impl Executor {
    pub fn new(
        docker: PlaneDocker,
        state_store: StateStore,
        ip: IpAddr,
        auto_prune: bool,
        cleanup_min_age: Duration,
    ) -> Self {
        let backends: Arc<DashMap<BackendName, Arc<BackendManager>>> = Arc::default();

        let backend_event_listener = {
            let docker = docker.clone();
            let backends = backends.clone();

            GuardHandle::new(async move {
                let mut events = docker.backend_events().await;
                while let Some(event) = events.next().await {
                    if let Some((_, manager)) = backends.remove(&event.backend_id) {
                        tracing::info!(
                            "Backend {} terminated with exit code {}.",
                            event.backend_id,
                            event.exit_code.unwrap_or(-1)
                        );

                        if let Err(err) = manager.mark_terminated(event.exit_code) {
                            tracing::error!(?err, "Error marking backend as terminated.");
                        }
                    }
                }

                tracing::info!("Backend event listener stopped.");
            })
        };

        let cleanup_handle = {
            let docker = docker.clone();
            GuardHandle::new(async move {
                cleanup_loop(
                    docker.clone(),
                    cleanup_min_age,
                    Duration::try_seconds(CLEANUP_INTERVAL_SECS).expect("duration is always valid"),
                    auto_prune,
                )
                .await;
            })
        };

        Self {
            docker,
            state_store: Arc::new(Mutex::new(state_store)),
            backends,
            ip,
            _backend_event_listener: backend_event_listener,
            _cleanup_handle: cleanup_handle,
        }
    }

    pub fn register_listener<F>(&self, listener: F) -> Result<()>
    where
        F: Fn(BackendStateMessage) + Send + Sync + 'static,
    {
        self.state_store
            .lock()
            .expect("State store lock poisoned.")
            .register_listener(listener)
    }

    pub fn register_metrics_sender(&self, sender: TypedSocketSender<BackendMetricsMessage>) {
        self.state_store
            .lock()
            .expect("State store lock poisoned")
            .register_metrics_sender(sender);
    }

    pub fn ack_event(&self, event_id: BackendEventId) -> Result<()> {
        self.state_store
            .lock()
            .expect("State store lock poisoned.")
            .ack_event(event_id)
    }

    pub async fn apply_action(
        &self,
        backend_id: &BackendName,
        action: &BackendAction,
    ) -> Result<()> {
        match action {
            BackendAction::Spawn {
                executable,
                key,
                static_token,
            } => {
                let callback = {
                    let state_store = self.state_store.clone();
                    let backend_id = backend_id.clone();
                    let timestamp = chrono::Utc::now();
                    move |state: &BackendState| {
                        state_store
                            .lock()
                            .expect("State store lock poisoned.")
                            .register_event(&backend_id, state, timestamp)?;

                        Ok(())
                    }
                };

                let metrics_sender = self
                    .state_store
                    .lock()
                    .expect("State store lock poisoned")
                    .get_metrics_sender()?;

                let manager = BackendManager::new(
                    backend_id.clone(),
                    executable.as_ref().clone(),
                    BackendState::default(),
                    self.docker.clone(),
                    callback,
                    metrics_sender,
                    self.ip,
                    key.clone(),
                    static_token.clone(),
                );
                tracing::info!("Inserting backend {}.", backend_id);
                self.backends.insert(backend_id.clone(), manager);
            }
            BackendAction::Terminate { kind, reason } => {
                tracing::info!("Terminating backend {}.", backend_id);

                let manager = {
                    // We need to be careful here not to hold the lock when we call terminate, or
                    // else we can deadlock.
                    let Some(manager) = self.backends.get(backend_id) else {
                        return Ok(());
                    };
                    manager.clone()
                };

                manager.terminate(*kind, *reason).await?;
            }
        }

        Ok(())
    }
}
