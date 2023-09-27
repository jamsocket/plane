use super::{
    backend::BackendMonitor,
    engine::{Engine, EngineBackendStatus},
};
use crate::{
    agent::wait_port_ready,
    database::{Backend, DroneDatabase},
};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use dashmap::DashMap;
use plane_core::{
    messages::{
        agent::{BackendState, DroneLogMessageKind, SpawnRequest, TerminationRequest},
        drone_state::UpdateBackendStateMessage,
    },
    nats::TypedNats,
    types::{BackendId, ClusterName, DroneId},
};
use serde_json::json;
use std::{fmt::Debug, net::IpAddr, sync::Arc};
use tokio::{
    sync::mpsc::{channel, Receiver, Sender},
    task::JoinHandle,
};
use tokio_stream::StreamExt;

trait LogError {
    fn log_error(&self) -> &Self;
}

impl<T, E: Debug> LogError for Result<T, E> {
    fn log_error(&self) -> &Self {
        match self {
            Ok(_) => (),
            Err(error) => tracing::error!(?error, "Encountered non-blocking error."),
        }

        self
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    /// Tells the executor to interrupt current step to recapture an external status
    /// change. This signal is sent after an engine detects an external status change,
    /// e.g. a backend has terminated itself with an error.
    Interrupt,

    /// Tells the executor to terminate the current step.
    Terminate,
}

pub struct Executor<E: Engine> {
    engine: Arc<E>,
    database: DroneDatabase,
    nc: TypedNats,
    _container_events_handle: Arc<JoinHandle<()>>,

    /// Associates a backend with a monitor, which owns a number of
    /// event loops related to a backend.
    backend_to_monitor: Arc<DashMap<BackendId, BackendMonitor>>,

    /// Associates a backend with a channel, through which signals can
    /// be sent to interrupt the state machine. This is used for
    /// telling the state machine to receive external events, and also
    /// for terminating backends.
    backend_to_listener: Arc<DashMap<BackendId, Sender<Signal>>>,

    /// The IP address associated with this executor.
    ip: IpAddr,

    /// The cluster name associated with this executor.
    cluster: ClusterName,
}

impl<E: Engine> Clone for Executor<E> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            database: self.database.clone(),
            nc: self.nc.clone(),
            _container_events_handle: self._container_events_handle.clone(),
            backend_to_monitor: self.backend_to_monitor.clone(),
            backend_to_listener: self.backend_to_listener.clone(),
            ip: self.ip,
            cluster: self.cluster.clone(),
        }
    }
}

async fn update_backend_state(
    nc: &TypedNats,
    state: BackendState,
    cluster: ClusterName,
    backend: BackendId,
    drone: DroneId,
) {
    let message = UpdateBackendStateMessage {
        state,
        cluster: cluster.clone(),
        backend: backend.clone(),
        time: Utc::now(),
        drone: drone.clone(),
    };
    let nc = nc.clone();

    while let Err(error) = nc.request(&message).await {
        tracing::error!(
            ?error,
            ?state,
            ?cluster,
            ?backend,
            ?drone,
            "Failed to update backend state, retrying."
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

impl<E: Engine> Executor<E> {
    pub fn new(
        engine: E,
        database: DroneDatabase,
        nc: TypedNats,
        ip: IpAddr,
        cluster: ClusterName,
    ) -> Self {
        let backend_to_listener: Arc<DashMap<BackendId, Sender<Signal>>> = Arc::default();
        let engine = Arc::new(engine);

        let container_events_handle = tokio::spawn(Self::listen_for_container_events(
            engine.clone(),
            backend_to_listener.clone(),
        ));

        Executor {
            engine,
            database,
            nc,
            _container_events_handle: Arc::new(container_events_handle),
            backend_to_monitor: Arc::default(),
            backend_to_listener,
            ip,
            cluster,
        }
    }

    async fn listen_for_container_events(
        engine: Arc<E>,
        backend_to_listener: Arc<DashMap<BackendId, Sender<Signal>>>,
    ) {
        let mut event_stream = engine.interrupt_stream();
        while let Some(backend_id) = event_stream.next().await {
            if let Some(v) = backend_to_listener.get(&backend_id) {
                v.try_send(Signal::Interrupt).log_error();
            }
        }
    }

    pub fn initialize_listener(&self, backend_id: BackendId, send: Sender<Signal>) {
        self.backend_to_listener.insert(backend_id, send);
    }

    pub async fn start_backend(&self, spawn_request: &SpawnRequest, recv: Receiver<Signal>) {
        self.database
            .insert_backend(spawn_request)
            .await
            .log_error();

        update_backend_state(
            &self.nc,
            BackendState::Loading,
            self.cluster.clone(),
            spawn_request.backend_id.clone(),
            spawn_request.drone_id.clone(),
        )
        .await;

        self.run_backend(spawn_request, BackendState::Loading, Some(recv))
            .await
    }

    pub async fn kill_backend(
        &self,
        termination_request: &TerminationRequest,
    ) -> Result<(), anyhow::Error> {
        if let Some(sender) = self
            .backend_to_listener
            .get(&termination_request.backend_id)
        {
            Ok(sender.send(Signal::Terminate).await?)
        } else {
            Err(anyhow!(
                "Unknown backend {}",
                &termination_request.backend_id
            ))
        }
    }

    pub async fn resume_backends(&self) -> Result<()> {
        let backends = self.database.get_backends().await?;

        for backend in backends {
            let executor = self.clone();
            let Backend {
                backend_id,
                state,
                spec,
            } = backend;
            tracing::info!(%backend_id, ?state, "Resuming backend");

            tokio::spawn(async move { executor.run_backend(&spec, state, None).await });
        }

        Ok(())
    }

    async fn run_backend(
        &self,
        spawn_request: &SpawnRequest,
        mut state: BackendState,
        recv_maybe: Option<Receiver<Signal>>,
    ) {
        let mut recv = recv_maybe.unwrap_or_else(|| {
            let (send, recv) = channel(1);
            self.backend_to_listener
                .insert(spawn_request.backend_id.clone(), send);
            recv
        });

        // the BackendMonitor must be created after the backend is ready
        // unless there's an error before the backend starts
        // this notifier is used to indicate that the BackendMonitor
        // should be created.
        let notify = Arc::new(tokio::sync::Notify::new());
        let jh = tokio::spawn({
            let notify = notify.clone();
            let spawn_request = spawn_request.clone();
            let s = self.clone();
            async move {
                notify.notified().await;
                let bm = BackendMonitor::new(
                    &spawn_request.backend_id,
                    &spawn_request.drone_id,
                    &s.cluster,
                    s.ip,
                    s.engine.as_ref(),
                    &s.nc,
                );
                s.backend_to_monitor.insert(spawn_request.backend_id, bm);
            }
        });

        loop {
            tracing::info!(
                ?state,
                backend_id = spawn_request.backend_id.id(),
                metadata = %json!(spawn_request.metadata),
                "Executing state."
            );

            if state.running() {
                //this creates the BackendMonitor
                notify.notify_one();
            }

            let next_state = loop {
                if state.terminal() {
                    //we ignore external state changes in a terminal state
                    //to avoid multiple destructor calls for backend.
                    break self.step(spawn_request, state).await;
                } else {
                    tokio::select! {
                        next_state = self.step(spawn_request, state) => break next_state,
                        sig = recv.recv() => match sig {
                            Some(Signal::Interrupt) => {
                                tracing::info!("State may have updated externally.");
                                continue;
                            },
                            Some(Signal::Terminate) => {
                                break Ok(Some(BackendState::Terminated))
                            },
                            None => {
                                tracing::error!("Signal sender lost!");
                                return
                            }
                        },
                    };
                }
            };

            match next_state {
                Ok(Some(new_state)) => {
                    if new_state.terminal() {
                        if let Err(inject_err) = self.inject_exit_code(spawn_request).await {
                            tracing::error!(
                                ?inject_err,
                                "failed to inject exit code for failed backend into logs"
                            );
                        }
                    }
                    state = new_state;
                    self.update_backend_state(spawn_request, state).await;
                }
                Ok(None) => {
                    // Successful termination.
                    tracing::info!("Backend terminated successfully.");
                    break;
                }
                Err(error) => {
                    tracing::error!(?error, ?state, "Encountered error.");
                    match state {
                        BackendState::Loading => {
                            state = BackendState::ErrorLoading;

                            //create the BackendMonitor to take errorloading logs
                            notify.notify_one();

                            //must wait on BackendMonitor's insertion into hashtable.
                            if jh.await.is_ok() {
                                if let Err(inject_err) = self
                                    .backend_to_monitor
                                    .try_get_mut(&spawn_request.backend_id)
                                    .unwrap() //this cannot fail since we insert into the table in jh
                                    .inject_log(error.to_string(), DroneLogMessageKind::Meta)
                                    .await
                                {
                                    tracing::error!(
                                        ?inject_err,
                                        "failed to inject error into logs"
                                    );
                                }
                            } else {
                                tracing::error!("inserting into backend_to_monitor failed");
                            }
                            self.update_backend_state(spawn_request, state).await;
                        }
                        _ => tracing::error!(
                            ?error,
                            ?state,
                            "Error unhandled (no change in backend state)"
                        ),
                    }
                    break;
                }
            }
        }

        self.backend_to_monitor.remove(&spawn_request.backend_id);
        self.backend_to_listener.remove(&spawn_request.backend_id);
    }

    async fn inject_exit_code(&self, spawn_request: &SpawnRequest) -> Result<()> {
        if let Ok(EngineBackendStatus::Failed { code }) =
            self.engine.backend_status(spawn_request).await
        {
            self.backend_to_monitor
                .try_get_mut(&spawn_request.backend_id)
                .try_unwrap()
                .ok_or(anyhow!("backend not in backend_to_monitor"))?
                .inject_log(
                    format!("Backend exit code: {}", code),
                    DroneLogMessageKind::Meta,
                )
                .await?;
        }
        Ok(())
    }

    /// Update the rest of the system on the state of a backend, by writing it to the local
    /// sqlite database (where the proxy can see it), and by broadcasting it to interested
    /// remote listeners over NATS.
    async fn update_backend_state(&self, spawn_request: &SpawnRequest, state: BackendState) {
        self.database
            .update_backend_state(&spawn_request.backend_id, state)
            .await
            .log_error();

        update_backend_state(
            &self.nc,
            state,
            self.cluster.clone(),
            spawn_request.backend_id.clone(),
            spawn_request.drone_id.clone(),
        )
        .await;
    }

    pub async fn step(
        &self,
        spawn_request: &SpawnRequest,
        state: BackendState,
    ) -> Result<Option<BackendState>> {
        match state {
            BackendState::Loading => {
                self.engine.load(spawn_request).await?;

                Ok(Some(BackendState::Starting))
            }
            BackendState::Starting => {
                let status = self.engine.backend_status(spawn_request).await?;

                let backend_addr = match status {
                    EngineBackendStatus::Running { addr } => addr,
                    _ => return Ok(Some(BackendState::ErrorStarting)),
                };

                tracing::info!(%backend_addr, "Got address from container.");
                if let Err(_err) = wait_port_ready(&backend_addr).await {
                    tracing::warn!(%spawn_request.backend_id, "backend timed out before ready");
                    return Ok(Some(BackendState::TimedOutBeforeReady));
                }

                self.database
                    .insert_proxy_route(
                        &spawn_request.backend_id,
                        spawn_request.backend_id.id(),
                        &backend_addr.to_string(),
                        spawn_request.bearer_token.as_deref(),
                    )
                    .await?;

                Ok(Some(BackendState::Ready))
            }
            BackendState::Ready => {
                match self.engine.backend_status(spawn_request).await? {
                    EngineBackendStatus::Failed { .. } => return Ok(Some(BackendState::Failed)),
                    EngineBackendStatus::Exited => return Ok(Some(BackendState::Exited)),
                    EngineBackendStatus::Terminated => return Ok(Some(BackendState::Swept)),
                    _ => (),
                }

                // wait for idle
                loop {
                    let last_active = self
                        .database
                        .get_backend_last_active(&spawn_request.backend_id)
                        .await?;
                    let next_check = last_active
                        .checked_add_signed(chrono::Duration::from_std(
                            spawn_request.max_idle_secs,
                        )?)
                        .context("Checked add error.")?;

                    if next_check < Utc::now() {
                        break;
                    } else {
                        tokio::time::sleep(next_check.signed_duration_since(Utc::now()).to_std()?)
                            .await;
                    }
                }

                Ok(Some(BackendState::Swept))
            }
            BackendState::ErrorLoading
            | BackendState::ErrorStarting
            | BackendState::TimedOutBeforeReady
            | BackendState::Failed
            | BackendState::Exited
            | BackendState::Swept
            | BackendState::Lost
            | BackendState::Terminated => {
                self.engine
                    .stop(&spawn_request.backend_id)
                    .await
                    .map_err(|e| anyhow!("Error stopping container: {:?}", e))?;

                Ok(None)
            }
        }
    }
}
