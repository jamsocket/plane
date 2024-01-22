use super::{
    docker::{get_metrics_message_from_container_stats, types::ContainerId, PlaneDocker},
    wait_backend::wait_for_backend,
};
use crate::{
    names::BackendName,
    protocol::BackendMetricsMessage,
    typed_socket::TypedSocketSender,
    types::{
        backend_state::TerminationReason, BackendState, BackendStatus, ExecutorConfig, PullPolicy,
        TerminationKind,
    },
    util::{ExponentialBackoff, GuardHandle},
};
use anyhow::Result;
use bollard::auth::DockerCredentials;
use futures_util::Future;
use std::{error::Error, fmt::Debug, sync::atomic::AtomicU64, time::Duration};
use std::{future::pending, pin::Pin};
use std::{
    net::IpAddr,
    sync::{Arc, Mutex, RwLock},
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

#[derive(Debug)]
struct MetricsManager {
    handle: Mutex<Option<GuardHandle>>,
    sender: Arc<RwLock<TypedSocketSender<BackendMetricsMessage>>>,
    docker: PlaneDocker,
    container_id: ContainerId,
    backend_id: BackendName,
    prev_container_cpu_cumulative_ns: Arc<AtomicU64>,
    prev_sys_cpu_cumulative_ns: Arc<AtomicU64>,
}

impl MetricsManager {
    fn new(
        sender: Arc<RwLock<TypedSocketSender<BackendMetricsMessage>>>,
        docker: PlaneDocker,
        container_id: ContainerId,
        backend_id: BackendName,
    ) -> Self {
        Self {
            handle: Mutex::new(None),
            sender,
            docker,
            container_id,
            backend_id,
            prev_container_cpu_cumulative_ns: Arc::new(AtomicU64::new(0)),
            prev_sys_cpu_cumulative_ns: Arc::new(AtomicU64::new(0)),
        }
    }

    fn start_gathering_metrics(&self) {
        let docker = self.docker.clone();
        let sender = self.sender.clone();
        let container_id = self.container_id.clone();
        let backend_id = self.backend_id.clone();
        let prev_sys_cpu_cumulative_ns = self.prev_sys_cpu_cumulative_ns.clone();
        let prev_container_cpu_cumulative_ns = self.prev_container_cpu_cumulative_ns.clone();
        let block = async move {
            let mut backoff = ExponentialBackoff::default();
            loop {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                while let Ok(stats) = {
                    interval.tick().await;
                    docker.get_metrics(&container_id).await
                } {
                    let Ok(metrics_message) = get_metrics_message_from_container_stats(
                        stats,
                        backend_id.clone(),
                        &prev_sys_cpu_cumulative_ns,
                        &prev_container_cpu_cumulative_ns,
                    ) else {
                        continue;
                    };
                    let _ = sender
                        .read()
                        .expect("failed to get read lock on metrics sender!")
                        .send(metrics_message);
                }
                backoff.wait().await;
            }
        };
        *self.handle.lock().expect("metrics handle lock poisoned") = Some(GuardHandle::new(block));
    }
}

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
pub struct BackendManager {
    state: Mutex<BackendManagerState>,

    /// handle for task that collects metrics and sends them to the controller.
    metrics_manager: MetricsManager,

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

impl Debug for BackendManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendManager")
            .field("backend_id", &self.backend_id)
            .field("container_id", &self.container_id)
            .finish()
    }
}

impl BackendManager {
    pub fn new(
        backend_id: BackendName,
        executor_config: ExecutorConfig,
        state: BackendState,
        docker: PlaneDocker,
        state_callback: impl Fn(&BackendState) -> Result<(), Box<dyn Error>> + Send + Sync + 'static,
        metrics_sender: Arc<RwLock<TypedSocketSender<BackendMetricsMessage>>>,
        ip: IpAddr,
    ) -> Arc<Self> {
        let container_id = ContainerId::from(format!("plane-{}", backend_id));

        let manager = Arc::new(Self {
            state: Mutex::new(BackendManagerState {
                state: state.clone(),
                handle: None,
            }),
            metrics_manager: MetricsManager::new(
                metrics_sender,
                docker.clone(),
                container_id.clone(),
                backend_id.clone(),
            ),
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
                let force_pull = match self.executor_config.pull_policy.unwrap_or_default() {
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
                    let credentials: Option<DockerCredentials> =
                        executor_config.credentials.map(|creds| creds.into());
                    tracing::info!(?image, "pulling...");
                    if let Err(err) = docker.pull(image, credentials.as_ref(), force_pull).await {
                        tracing::error!(?err, "failed to pull image");
                        state.to_terminated(None)
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
                self.metrics_manager.start_gathering_metrics();

                StepStatusResult::future_status(async move {
                    let spawn_result = docker
                        .spawn_backend(&backend_id, &container_id, executor_config)
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
            BackendStatus::Waiting => StepStatusResult::future_status(async move {
                let address = match state.address() {
                    Some(address) => address,
                    None => {
                        tracing::error!("State is waiting, but no associated address.");
                        return state.to_terminated(None);
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

    pub fn set_state(self: &Arc<Self>, state: BackendState) {
        tracing::info!(?self.backend_id, ?state, "Updating backend state");
        let mut lock = self.state.lock().expect("State lock is poisoned");

        tracing::info!(
            backend_id = self.backend_id.as_value(),
            state = state.as_value(),
            "Updating backend state"
        );

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
        self.set_state(state.to_terminated(exit_code));

        Ok(())
    }
}
