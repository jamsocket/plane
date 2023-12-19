use super::{
    docker::{types::ContainerId, PlaneDocker},
    wait_backend::wait_for_backend,
};
use crate::{
    names::BackendName,
    types::{BackendState, BackendStatus, ExecutorConfig, PullPolicy, TerminationKind},
    util::{ExponentialBackoff, GuardHandle},
};
use anyhow::Result;
use bollard::auth::DockerCredentials;
use futures_util::Future;
use std::error::Error;
use std::{future::pending, pin::Pin};
use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

/// The backend manager uses a state machine internally to manage the state of the backend.
/// Each time we enter a state, we can do one of three things:
/// - Do nothing (wait for an external event to trigger the next state change).
/// - Immediately jump to a new state.
/// - Start an async task that will eventually jump to a new state, unless an external event
///   triggers a state change first.
enum StepStatusResult {
    DoNothing,
    SetState(BackendState),
    FutureSetState(Pin<Box<dyn Future<Output = BackendState> + Send>>),
}

impl StepStatusResult {
    pub fn future_status<F>(future: F) -> Self
    where
        F: Future<Output = BackendState> + Send + 'static,
    {
        Self::FutureSetState(Box::pin(future))
    }
}

type StateCallback = Box<dyn Fn(&BackendState) -> Result<(), Box<dyn Error>> + Send + Sync>;

/// A backend manager is responsible for driving the state of one backend.
/// Every active backend should have a backend manager.
/// All container- and image-level commands sent to Docker go through the backend manager.
/// The backend manager owns the status for the backend it is responsible for.
pub struct BackendManager {
    /// If we are currently running a task, this is the handle to that task.
    /// It is always dropped (and aborted) when the state changes.
    handle: Mutex<Option<GuardHandle>>,

    /// The ID of the backend this manager is responsible for.
    backend_id: BackendName,

    /// The Docker client to use for all Docker operations.
    docker: PlaneDocker,

    /// The configuration to use when spawning the backend.
    executor_config: ExecutorConfig,

    /// Function to call when the state changes.
    state_callback: StateCallback,

    /// The ID of the container running the backend.
    container_id: ContainerId,

    /// IP address of the drone.
    ip: IpAddr,
}

impl BackendManager {
    pub fn new(
        backend_id: BackendName,
        executor_config: ExecutorConfig,
        state: BackendState,
        docker: PlaneDocker,
        state_callback: impl Fn(&BackendState) -> Result<(), Box<dyn Error>> + Send + Sync + 'static,
        ip: IpAddr,
    ) -> Arc<Self> {
        let container_id = ContainerId::from(format!("plane-{}", backend_id));

        let manager = Arc::new(Self {
            handle: Mutex::new(None),
            backend_id,
            docker,
            executor_config,
            state_callback: Box::new(state_callback),
            container_id,
            ip,
        });

        manager.set_state(state);
        manager
    }

    fn step_state(&self, state: BackendState) -> StepStatusResult {
        match state.status {
            BackendStatus::Scheduled => StepStatusResult::SetState(state.to_loading()),
            BackendStatus::Loading => {
                let force_pull = match self.executor_config.pull_policy {
                    PullPolicy::IfNotPresent => false,
                    PullPolicy::Always => true,
                    PullPolicy::Never => {
                        // Skip the loading step.
                        return StepStatusResult::SetState(state.to_starting());
                    }
                };

                let executor_config = self.executor_config.clone();
                let docker = self.docker.clone();
                StepStatusResult::future_status(async move {
                    let image = &executor_config.image;
                    let credentials: Option<&DockerCredentials> =
                        executor_config.credentials.as_ref();
                    tracing::info!(?image, "pulling...");
                    if let Err(err) = docker.pull(image, credentials, force_pull).await {
                        tracing::error!(?err, "failed to pull image");
                        BackendState::terminated(None)
                    } else {
                        tracing::info!("done pulling...");
                        state.to_starting()
                    }
                })
            }
            BackendStatus::Starting => {
                let backend_id = self.backend_id.clone();
                let container_id = self.container_id.clone();
                let docker = self.docker.clone();
                let executor_config = self.executor_config.clone();
                let ip = self.ip;

                StepStatusResult::future_status(async move {
                    let spawn_result = docker
                        .spawn_backend(&backend_id, &container_id, executor_config)
                        .await;

                    let spawn_result = match spawn_result {
                        Ok(spawn_result) => spawn_result,
                        Err(err) => {
                            tracing::error!(?err, "failed to spawn backend");
                            return BackendState::terminated(None);
                        }
                    };

                    let address = (ip, spawn_result.port).into();
                    state.to_waiting(address)
                })
            }
            BackendStatus::Waiting => StepStatusResult::future_status(async move {
                let address = match state.address {
                    Some(address) => address,
                    None => {
                        tracing::error!("State is waiting, but no associated address.");
                        return BackendState::terminated(None);
                    }
                };

                wait_for_backend(address).await;

                state.to_ready()
            }),
            BackendStatus::Ready => StepStatusResult::DoNothing,
            BackendStatus::Terminating => {
                let docker = self.docker.clone();
                let container_id = self.container_id.clone();

                StepStatusResult::future_status(async move {
                    let mut backoff = ExponentialBackoff::default();

                    loop {
                        match docker
                            .terminate_backend(
                                &container_id,
                                state.termination == Some(TerminationKind::Hard),
                            )
                            .await
                        {
                            Ok(()) => break,
                            Err(err) => {
                                tracing::error!(?err, "failed to terminate backend");
                                backoff.wait().await;
                            }
                        }
                    }

                    // Return a future that never resolves, so that only the container
                    // terminating bumps us into the next state.
                    pending().await
                })
            }
            BackendStatus::Terminated => StepStatusResult::DoNothing,
        }
    }

    pub fn set_state(self: &Arc<Self>, status: BackendState) {
        tracing::info!(?self.backend_id, ?status, "Updating backend state");
        let mut handle = self.handle.lock().expect("Guard handle lock is poisoned");
        // Cancel any existing task.
        handle.take();

        // Call the callback.
        if let Err(err) = (self.state_callback)(&status) {
            tracing::error!(?err, "Error calling state callback.");
            return;
        }

        let result = self.step_state(status);
        match result {
            StepStatusResult::DoNothing => {}
            StepStatusResult::SetState(status) => {
                // We need to drop the lock before we call ourselves recursively!
                drop(handle);
                self.set_state(status);
            }
            StepStatusResult::FutureSetState(future) => {
                let self_clone = self.clone();
                *handle = Some(GuardHandle::new(async move {
                    let status = future.await;
                    self_clone.set_state(status);
                }));
            }
        }
    }

    pub async fn terminate(self: &Arc<Self>, kind: TerminationKind) -> Result<()> {
        self.set_state(BackendState::terminating(kind));

        Ok(())
    }

    pub fn mark_terminated(self: &Arc<Self>, exit_code: Option<i32>) -> Result<()> {
        self.set_state(BackendState::terminated(exit_code));

        Ok(())
    }
}
