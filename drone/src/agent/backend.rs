use crate::agent::engine::Engine;
use futures::Future;
use plane_core::{
    logging::LogError,
    messages::agent::DroneLogMessage,
    messages::{
        agent::DroneLogMessageKind,
        dns::{DnsRecordType, SetDnsRecord},
    },
    nats::TypedNats,
    types::{BackendId, ClusterName},
};
use std::{net::IpAddr, time::Duration, ops::DerefMut};
use std::sync::{Arc, Mutex};
use tokio::{
    sync::{mpsc::{self, error::SendError, Sender, Receiver}, Barrier},
    task::JoinHandle,
    time::sleep,
};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};

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
    _backend_id: BackendId,
	meta_log_tx: Sender<DroneLogMessage>,
}

impl BackendMonitor {
    pub async fn new<E: Engine>(
        backend_id: &BackendId,
        cluster: &ClusterName,
        ip: IpAddr,
        engine: &E,
        nc: &TypedNats,
		notify: &tokio::sync::Notify
    ) -> Self {
		notify.notified().await;
		let (meta_tx, meta_rx) = tokio::sync::mpsc::channel(16);
		tracing::info!("who are you blocking??");
        let log_loop = Self::log_loop(backend_id, engine, nc,ReceiverStream::new(meta_rx));
		tracing::info!("2 who are you blocking??");
        let stats_loop = Self::stats_loop(backend_id, cluster, engine, nc);
		tracing::info!("3 who are you blocking??");
        let dns_loop = Self::dns_loop(backend_id, ip, nc, cluster);

        BackendMonitor {
            _log_loop: AbortOnDrop(log_loop),
            _stats_loop: AbortOnDrop(stats_loop),
            _dns_loop: AbortOnDrop(dns_loop),
            _backend_id: backend_id.to_owned(),
			meta_log_tx: meta_tx,
        }
    }

    pub fn inject_log(
        &mut self,
        text: String,
        kind: DroneLogMessageKind,
    ) -> impl Future<Output = Result<(), SendError<DroneLogMessage>>> + '_ {
        self.meta_log_tx.send(DroneLogMessage {
            backend_id: self._backend_id.clone(),
            kind,
            text,
        })
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
        meta_log_rx: ReceiverStream<DroneLogMessage>,
    ) -> JoinHandle<()> {
        let mut stream = engine.log_stream(backend_id).merge(meta_log_rx);
        let nc = nc.clone();
        let backend_id = backend_id.clone();

        tokio::spawn(async move {
            tracing::info!(%backend_id, "Log recording loop started.");

            while let Some(v) = stream.next().await {
				tracing::info!(%backend_id, ?v, "log message");
                nc.publish(&v)
                    .await
                    .log_error("Error publishing log message.");
            }

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
