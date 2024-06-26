use crate::drone::runtime::Runtime;
use crate::{
    names::BackendName,
    protocol::AcquiredKey,
    types::{
        backend_state::{BackendError, TerminationReason},
        BackendState, BearerToken, TerminationKind,
    },
    util::{ExponentialBackoff, GuardHandle},
};
use anyhow::Result;
use futures_util::Future;
use std::{error::Error, fmt::Debug};
use std::{future::pending, pin::Pin};
use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};
use valuable::Valuable;

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

struct BackendManagerState {
    /// The current state of the backend.
    state: BackendState,

    /// If we are currently running a task, this is the handle to that task.
    /// It is always dropped (and aborted) when the state changes.
    handle: Option<GuardHandle>,
}

/// A backend manager is responsible for driving the state of one backend.
/// Every active backend should have a backend manager.
/// All container- and image-level commands sent to Docker go through the backend manager.
/// The backend manager owns the status for the backend it is responsible for.
pub struct BackendManager<R: Runtime> {
    state: Mutex<BackendManagerState>,

    /// The ID of the backend this manager is responsible for.
    backend_id: BackendName,

    /// The Docker client to use for all Docker operations.
    runtime: Arc<R>,

    /// The configuration to use when spawning the backend.
    backend_config: R::BackendConfig,

    /// Function to call when the state changes.
    state_callback: StateCallback,

    /// IP address of the drone.
    ip: IpAddr,

    /// Key acquired by the backend.
    acquired_key: AcquiredKey,

    /// Static token to use for the backend.
    static_token: Option<BearerToken>,
}

impl<R: Runtime> Debug for BackendManager<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendManager")
            .field("backend_id", &self.backend_id)
            .finish()
    }
}

impl<R: Runtime> BackendManager<R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend_id: BackendName,
        backend_config: R::BackendConfig,
        state: BackendState,
        runtime: Arc<R>,
        state_callback: impl Fn(&BackendState) -> Result<(), Box<dyn Error>> + Send + Sync + 'static,
        ip: IpAddr,
        acquired_key: AcquiredKey,
        static_token: Option<BearerToken>,
    ) -> Arc<Self> {
        let manager = Arc::new(Self {
            state: Mutex::new(BackendManagerState {
                state: state.clone(),
                handle: None,
            }),
            backend_id,
            runtime,
            backend_config,
            state_callback: Box::new(state_callback),
            ip,
            acquired_key,
            static_token,
        });

        manager.set_state(state);
        manager
    }

    fn step_state(&self, state: BackendState) -> StepStatusResult {
        match state {
            BackendState::Scheduled => StepStatusResult::SetState(state.to_loading()),
            BackendState::Loading => {
                let executor_config = self.backend_config.clone();
                let runtime = self.runtime.clone();
                let backend_id = self.backend_id.clone();
                StepStatusResult::future_status(async move {
                    tracing::info!(%backend_id, "preparing...");
                    if let Err(err) = runtime.prepare(&executor_config).await {
                        tracing::error!(?err, %backend_id, "failed to prepare");
                        state.to_terminated(None)
                    } else {
                        tracing::info!(%backend_id, "done preparing...");
                        state.to_starting()
                    }
                })
            }
            BackendState::Starting => {
                let backend_id = self.backend_id.clone();
                let runtime = self.runtime.clone();
                let executor_config = self.backend_config.clone();
                let ip = self.ip;

                let acquired_key = self.acquired_key.clone();
                let static_token = self.static_token.clone();
                StepStatusResult::future_status(async move {
                    let spawn_result = runtime
                        .spawn(
                            &backend_id,
                            executor_config,
                            Some(&acquired_key),
                            static_token.as_ref(),
                        )
                        .await;

                    let spawn_result = match spawn_result {
                        Ok(spawn_result) => spawn_result,
                        Err(err) => {
                            tracing::error!(?err, "failed to spawn backend");
                            return state.to_terminated(None);
                        }
                    };

                    let address = (ip, spawn_result.port).into();
                    state.to_waiting(address)
                })
            }
            BackendState::Waiting { address } => {
                let backend_id = self.backend_id.clone();
                let runtime = self.runtime.clone();
                StepStatusResult::future_status(async move {
                    if let Err(BackendError::StartupTimeout) =
                        runtime.wait_for_backend(&backend_id, address.0).await
                    {
                        tracing::error!("Backend startup timeout");
                        state.to_terminating(
                            TerminationKind::Hard,
                            TerminationReason::StartupTimeout,
                        )
                    } else {
                        state.to_ready()
                    }
                })
            }
            BackendState::Ready { .. } => StepStatusResult::DoNothing,
            BackendState::Terminating { termination, .. } => {
                let docker = self.runtime.clone();
                let backend_id = self.backend_id.clone();

                StepStatusResult::future_status(async move {
                    let mut backoff = ExponentialBackoff::default();

                    loop {
                        match docker
                            .terminate(&backend_id, termination == TerminationKind::Hard)
                            .await
                        {
                            Ok(_) => break,
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
            BackendState::Terminated { .. } => StepStatusResult::DoNothing,
        }
    }

    pub fn set_state(self: &Arc<Self>, state: BackendState) {
        let mut lock = self.state.lock().expect("State lock is poisoned");

        tracing::info!(
            backend_id = self.backend_id.as_value(),
            state = state.as_value(),
            "Updating backend state"
        );

        lock.state = state.clone();

        // Cancel any existing task.
        lock.handle.take();

        // Call the callback.
        if let Err(err) = (self.state_callback)(&state) {
            tracing::error!(?err, "Error calling state callback.");
            return;
        }

        let result = self.step_state(state);
        match result {
            StepStatusResult::DoNothing => {}
            StepStatusResult::SetState(status) => {
                // We need to drop the lock before we call ourselves recursively!
                drop(lock);
                self.set_state(status);
            }
            StepStatusResult::FutureSetState(future) => {
                let self_clone = self.clone();
                lock.handle = Some(GuardHandle::new(async move {
                    let status = future.await;
                    self_clone.set_state(status);
                }));
            }
        }
    }

    pub async fn terminate(
        self: &Arc<Self>,
        kind: TerminationKind,
        reason: TerminationReason,
    ) -> Result<()> {
        let state = self
            .state
            .lock()
            .expect("State lock is poisoned")
            .state
            .clone();
        self.set_state(state.to_terminating(kind, reason));

        Ok(())
    }

    pub fn mark_terminated(self: &Arc<Self>, exit_code: Option<i32>) -> Result<()> {
        let state = self
            .state
            .lock()
            .expect("State lock is poisoned")
            .state
            .clone();
        tracing::info!(
            backend_id = self.backend_id.as_value(),
            state = state.as_value(),
            "Marking backend as terminated"
        );
        self.set_state(state.to_terminated(exit_code));

        Ok(())
    }
}
