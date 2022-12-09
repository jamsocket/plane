use crate::agent::engine::Engine;
use plane_core::{logging::LogError, nats::TypedNats, types::BackendId, AbortOnDrop};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

pub struct BackendMonitor {
    _log_loop: AbortOnDrop,
    _stats_loop: AbortOnDrop,
}

impl BackendMonitor {
    pub fn new<E: Engine>(backend_id: &BackendId, engine: &E, nc: &TypedNats) -> Self {
        let log_loop = Self::log_loop(backend_id, engine, nc);
        let stats_loop = Self::stats_loop(backend_id, engine, nc);

        BackendMonitor {
            _log_loop: AbortOnDrop::new(log_loop),
            _stats_loop: AbortOnDrop::new(stats_loop),
        }
    }

    fn log_loop<E: Engine>(backend_id: &BackendId, engine: &E, nc: &TypedNats) -> JoinHandle<()> {
        let mut stream = engine.log_stream(backend_id);
        let nc = nc.clone();
        let backend_id = backend_id.clone();

        tokio::spawn(async move {
            tracing::info!(%backend_id, "Log recording loop started.");

            while let Some(v) = stream.next().await {
                nc.publish(&v)
                    .await
                    .log_error("Error publishing log message.");
            }

            tracing::info!(%backend_id, "Log loop terminated.");
        })
    }

    fn stats_loop<E: Engine>(backend_id: &BackendId, engine: &E, nc: &TypedNats) -> JoinHandle<()> {
        let mut stream = Box::pin(engine.stats_stream(backend_id));
        let nc = nc.clone();
        let backend_id = backend_id.clone();

        tokio::spawn(async move {
            tracing::info!(%backend_id, "Stats recording loop started.");

            while let Some(stats) = stream.next().await {
                nc.publish(&stats)
                    .await
                    .log_error("Error publishing stats message.");
            }

            tracing::info!(%backend_id, "Stats loop terminated.");
        })
    }
}
