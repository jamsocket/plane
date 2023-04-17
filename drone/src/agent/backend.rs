use crate::agent::engine::Engine;
use plane_core::{
    logging::LogError,
    messages::agent::DroneLogMessage,
    messages::dns::{DnsRecordType, SetDnsRecord},
    nats::TypedNats,
    types::{BackendId, ClusterName},
};
use std::{net::IpAddr, time::Duration};
use tokio::{
    sync::mpsc::{self, Receiver},
    task::JoinHandle,
    time::sleep,
};
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
    _dns_loop: AbortOnDrop<Result<(), anyhow::Error>>,
    pub inject_log: Box<dyn FnMut(String) + Send + Sync>,
}

impl BackendMonitor {
    pub fn new<E: Engine>(
        backend_id: &BackendId,
        cluster: &ClusterName,
        ip: IpAddr,
        engine: &E,
        nc: &TypedNats,
    ) -> Self {
        let (meta_log_tx, mut meta_log_rx) = mpsc::channel(16);
        let log_loop = Self::log_loop(backend_id, engine, nc, meta_log_rx);
        let stats_loop = Self::stats_loop(backend_id, cluster, engine, nc);
        let dns_loop = Self::dns_loop(backend_id, ip, nc, cluster);

        BackendMonitor {
            _log_loop: AbortOnDrop(log_loop),
            _stats_loop: AbortOnDrop(stats_loop),
            _dns_loop: AbortOnDrop(dns_loop),
            inject_log: Box::new(|text: String| { meta_log_tx.clone().send(DroneLogMessage {
				backend_id: backend_id.to_owned(),
				kind: plane_core::messages::agent::DroneLogMessageKind::Stderr,
				text
			}); }),
        }
    }

    fn dns_loop(
        backend_id: &BackendId,
        ip: IpAddr,
        nc: &TypedNats,
        cluster: &ClusterName,
    ) -> JoinHandle<Result<(), anyhow::Error>> {
        let backend_id = backend_id.clone();
        let nc = nc.clone();
        let cluster = cluster.clone();

        tokio::spawn(async move {
            loop {
                nc.publish(&SetDnsRecord {
                    cluster: cluster.clone(),
                    kind: DnsRecordType::A,
                    name: backend_id.to_string(),
                    value: ip.to_string(),
                })
                .await
                .log_error("Error publishing DNS record.");

                sleep(Duration::from_secs(SetDnsRecord::send_period())).await;
            }
        })
    }

    fn log_loop<E: Engine>(
        backend_id: &BackendId,
        engine: &E,
        nc: &TypedNats,
        mut meta_log_rx: Receiver<DroneLogMessage>,
    ) -> JoinHandle<()> {
        let mut stream = engine.log_stream(backend_id);
        let nc = nc.clone();
        let backend_id = backend_id.clone();

        tokio::spawn(async move {
            tracing::info!(%backend_id, "Log recording loop started.");

            let v = tokio::select! {
                Some(v) = stream.next() => { v }
                Some(v) = meta_log_rx.recv() => { v }
            };

            nc.publish(&v)
                .await
                .log_error("Error publishing log message.");

            tracing::info!(%backend_id, "Log loop terminated.");
        })
    }

    fn stats_loop<E: Engine>(
        backend_id: &BackendId,
        cluster: &ClusterName,
        engine: &E,
        nc: &TypedNats,
    ) -> JoinHandle<()> {
        let mut stream = Box::pin(engine.stats_stream(backend_id, cluster));
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
