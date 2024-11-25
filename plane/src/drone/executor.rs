use crate::util::GuardHandle;

use super::{backend_manager::BackendManager, runtime::Runtime, state_store::StateStore};
use anyhow::Result;
use chrono::Utc;
use dashmap::DashMap;
use futures_util::{future::join_all, StreamExt};
use plane_client::{
    exponential_backoff::ExponentialBackoff,
    names::BackendName,
    protocol::{BackendAction, BackendEventId, BackendStateMessage},
    types::{BackendState, BackendStatus, TerminationReason},
};
use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};
use valuable::Valuable;

pub struct Executor {
    pub runtime: Arc<Box<dyn Runtime>>,
    state_store: Arc<Mutex<StateStore>>,
    backends: Arc<DashMap<BackendName, Arc<BackendManager>>>,
    ip: IpAddr,
    _backend_event_listener: GuardHandle,
}

impl Executor {
    pub async fn new(runtime: Arc<Box<dyn Runtime>>, state_store: StateStore, ip: IpAddr) -> Self {
        let backends: Arc<DashMap<BackendName, Arc<BackendManager>>> = Arc::default();
        let state_store = Arc::new(Mutex::new(state_store));

        #[allow(clippy::unwrap_used)]
        Self::terminate_preexisting_backends(runtime.clone(), state_store.clone())
            .await
            .expect("Failed to terminate all preexisting backends! Locks may be violated, Drone aborting startup.");

        let backend_event_listener = {
            let docker = runtime.clone();
            let backends = backends.clone();

            GuardHandle::new(async move {
                let mut events = docker.events();
                while let Some(event) = events.next().await {
                    if let Some((_, manager)) = backends.remove(&event.backend_id) {
                        tracing::info!(
                            backend_id = event.backend_id.as_value(),
                            exit_code = event.exit_code.unwrap_or(-1),
                            "Backend terminated.",
                        );

                        if let Err(err) = manager.mark_terminated(event.exit_code) {
                            tracing::error!(?err, "Error marking backend as terminated.");
                        }
                    }
                }

                tracing::info!("Backend event listener stopped.");
            })
        };

        Self {
            runtime,
            state_store,
            backends,
            ip,
            _backend_event_listener: backend_event_listener,
        }
    }

    // On restart, we want to terminate all existing backends and start fresh.
    // This prevents bugs where an agent restart leaves the drone unable to
    // terminate old backends.
    async fn terminate_preexisting_backends(
        runtime: Arc<Box<dyn Runtime>>,
        state_store: Arc<Mutex<StateStore>>,
    ) -> Result<()> {
        let backends = state_store
            .lock()
            .expect("State store lock poisoned.")
            .active_backends()?;

        if !backends.is_empty() {
            tracing::info!(?backends, "Terminating preexisting backends");
        }
        let mut tasks = vec![];
        for (backend_id, state) in backends {
            let runtime = runtime.clone();
            let state_store = state_store.clone();
            let state = state.clone();
            tasks.push(async move {
                state_store
                    .lock()
                    .expect("State store lock poisoned.")
                    .register_event(
                        &backend_id,
                        &state.to_hard_terminating(TerminationReason::KeyExpired),
                        Utc::now(),
                    )
                    .unwrap_or_else(|_| {
                        panic!(
                            "Failed to register backend terminating for backend {:?}",
                            backend_id
                        )
                    });

                let mut backoff = ExponentialBackoff::default();
                let mut success = false;
                for attempt in 1..=10 {
                    match runtime.terminate(&backend_id, true).await {
                        Ok(_) => {
                            success = true;
                            break;
                        }
                        Err(err) => {
                            tracing::warn!(
                                ?err,
                                ?backend_id,
                                ?attempt,
                                "Attempt failed to terminate backend"
                            );
                            backoff.wait().await;
                        }
                    }
                }
                if !success {
                    tracing::warn!(
                        ?backend_id,
                        "Failed to terminate backend after 10 attempts. Marking terminated anyways."
                    );
                }
                state_store
                    .lock()
                    .expect("State store lock poisoned.")
                    .register_event(&backend_id, &state.to_terminated(None), Utc::now())
                    .unwrap_or_else(|_| {
                        panic!(
                            "Failed to register backend termination for backend {:?}",
                            backend_id
                        )
                    });
            });
        }

        join_all(tasks).await;

        Ok(())
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
                    move |state: &BackendState| {
                        let timestamp = chrono::Utc::now();
                        state_store
                            .lock()
                            .expect("State store lock poisoned.")
                            .register_event(&backend_id, state, timestamp)?;

                        Ok(())
                    }
                };

                let manager = BackendManager::new(
                    backend_id.clone(),
                    executable.clone(),
                    BackendState::default(),
                    self.runtime.clone(),
                    callback,
                    self.ip,
                    key.clone(),
                    static_token.clone(),
                );
                tracing::info!(backend_id = backend_id.as_value(), "Inserting backend.");
                self.backends.insert(backend_id.clone(), manager);
            }
            BackendAction::Terminate { kind, reason } => {
                tracing::info!("Terminating backend {}.", backend_id);

                let manager = {
                    // We need to be careful here not to hold the lock when we call terminate, or
                    // else we can deadlock.
                    let Some(manager) = self.backends.get(backend_id) else {
                        tracing::warn!(backend_id = backend_id.as_value(), "Backend not found when handling terminate action (assumed terminated).");

                        // Terminate will only return an error on a docker error, not if the backend is already terminated.
                        self.runtime.terminate(backend_id, true).await?;

                        self.state_store
                            .lock()
                            .expect("State store lock poisoned.")
                            .register_event(
                                backend_id,
                                &BackendState::Terminated {
                                    last_status: BackendStatus::Ready, // assumed
                                    termination: None,
                                    reason: Some(TerminationReason::Lost),
                                    exit_code: None,
                                },
                                Utc::now(),
                            )?;

                        return Ok(());
                    };
                    manager.clone()
                };

                manager.terminate(*kind, *reason).await;
            }
        }

        Ok(())
    }
}
