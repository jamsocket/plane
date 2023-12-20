use super::{backend_manager::BackendManager, docker::PlaneDocker, state_store::StateStore};
use crate::{
    names::BackendName,
    protocol::{BackendAction, BackendEventId, BackendStateMessage, BackendMetricsMessage},
    types::BackendState,
    util::GuardHandle,
};
use std::error::Error;
use anyhow::Result;
use bollard::container::Stats;
use dashmap::DashMap;
use futures_util::StreamExt;
use std::{
    net::IpAddr,
    sync::{Arc, Mutex, OnceLock},
};


pub struct MetricsListener(Box<dyn Fn(BackendMetricsMessage) + Send + Sync + 'static>);
impl MetricsListener {
	fn new(value: impl Fn(BackendMetricsMessage) + Send + Sync + 'static) -> Self {
		Self(Box::new(value))
	}

	fn call(&self, msg: BackendMetricsMessage) {
		self.0(msg)
	}
}

pub struct Executor {
    docker: PlaneDocker,
    state_store: Arc<Mutex<StateStore>>,
	metrics_listener: Arc<OnceLock<MetricsListener>>,
    backends: Arc<DashMap<BackendName, Arc<BackendManager>>>,
    ip: IpAddr,
    _backend_event_listener: GuardHandle,
}

impl Executor {
    pub fn new(docker: PlaneDocker, state_store: StateStore, ip: IpAddr) -> Self {
        let backends: Arc<DashMap<BackendName, Arc<BackendManager>>> = Arc::default();

        let backend_event_listener = {
            let docker = docker.clone();
            let backends = backends.clone();

            GuardHandle::new(async move {
                let mut events = docker.backend_events().await;
                while let Some(event) = events.next().await {
                    tracing::info!("Received backend event!!: {:?}", event);
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

        Self {
            docker,
            state_store: Arc::new(Mutex::new(state_store)),
			metrics_listener: Arc::new(OnceLock::new()),
            backends,
            ip,
            _backend_event_listener: backend_event_listener,
        }
    }

    pub fn register_listener<F>(&mut self, listener: F) -> Result<()>
    where
        F: Fn(BackendStateMessage) + Send + Sync + 'static,
    {
        self.state_store
            .lock()
            .expect("State store lock poisoned.")
            .register_listener(listener)
    }

	pub fn register_metrics_listener(&mut self, listener: impl Fn(BackendMetricsMessage) + Send + Sync + 'static) -> Result<()> {
		self.metrics_listener.set(MetricsListener::new(listener)).map_err(|e| anyhow::anyhow!("cant!"))
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
            BackendAction::Spawn { executable, .. } => {
                let callback = {
                    let state_store = self.state_store.clone();
                    let backend_id = backend_id.clone();
                    let timestamp = chrono::Utc::now();
                    move |state: BackendState| {
                        state_store
                            .lock()
                            .expect("State store lock poisoned.")
                            .register_event(&backend_id, &state, timestamp)?;

                        Ok(())
                    }
                };

				let metrics_callback = {
					let metrics_listener = self.metrics_listener.clone();
                    let backend_id = backend_id.clone();
					move |metrics: Stats| {
						let Some(listener) = metrics_listener.get() else {
							return Ok(())
						};
						listener.call( BackendMetricsMessage {
								backend_id: backend_id.clone()
						});

						Ok(())
					}
				};

                let manager = BackendManager::new(
                    backend_id.clone(),
                    executable.clone(),
                    BackendState::default(),
                    self.docker.clone(),
                    callback,
					metrics_callback, 
                    self.ip,
                );
                tracing::info!("Inserting backend {}.", backend_id);
                self.backends.insert(backend_id.clone(), manager);
            }
            BackendAction::Terminate { kind } => {
                tracing::info!("Terminating backend {}.", backend_id);

                let manager = {
                    // We need to be careful here not to hold the lock when we call terminate, or
                    // else we can deadlock.
                    let Some(manager) = self.backends.get(backend_id) else {
                        return Ok(());
                    };
                    manager.clone()
                };

                manager.terminate(*kind).await?;
            }
        }

        Ok(())
    }
}
