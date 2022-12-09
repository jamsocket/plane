use crate::agent::engine::Engine;
use plane_core::{logging::LogError, nats::TypedNats, types::BackendId};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

/// JoinHandle does not abort when it is dropped; this wrapper does.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct BackendMonitor {
    _log_loop: AbortOnDrop<()>,
    _stats_loop: AbortOnDrop<()>,
}

impl BackendMonitor {
    pub fn new<E: Engine>(backend_id: &BackendId, engine: &E, nc: &TypedNats) -> Self {
        let log_loop = Self::log_loop(backend_id, engine, nc);
        let stats_loop = Self::stats_loop(backend_id, engine, nc);

        BackendMonitor {
            _log_loop: AbortOnDrop(log_loop),
            _stats_loop: AbortOnDrop(stats_loop),
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
