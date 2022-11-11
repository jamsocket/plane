use super::engine::BackendEngine;
use crate::database::DroneDatabase;
use anyhow::Result;
use plane_core::{nats::TypedNats, types::BackendId};
use tokio::{task::JoinHandle, sync::oneshot::{Sender, channel}};
use std::{fmt::Debug, collections::HashMap, sync::Arc};

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

pub struct Backend {
    task: JoinHandle<()>,
    terminate_sender: Sender<()>,
}

impl Backend {
    pub fn run<E: BackendEngine>(engine: Arc<E>, spec: E::Spec) -> Self {
        let (terminate_sender, terminate_receiver) = channel();

        let task = tokio::spawn(async {
            engine.load(&spec).await;
        });

        Backend {
            task,
            terminate_sender,
        }
    }
}

pub struct Executor<E: BackendEngine> {
    engine: Arc<E>,
    database: DroneDatabase,
    nc: TypedNats,
    backend_tasks: HashMap<BackendId, Backend>,
}

impl<E: BackendEngine> Executor<E> {
    pub fn new(engine: E, database: DroneDatabase, nc: TypedNats) -> Self {
        // TODO: load existing backends from database

        Executor {
            engine: Arc::new(engine),
            database,
            nc,
            backend_tasks: HashMap::default(),
        }
    }

    pub fn terminate(&self, backend: &BackendId) {
        todo!("Terminate not implemented yet.")
    }

    pub fn run_backend(&mut self, backend_id: &BackendId, spec: E::Spec) {
        self.backend_tasks.insert(backend_id.clone(), Backend::run(self.engine.clone(), spec));
    }
}
