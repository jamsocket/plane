use crate::agent::engines::docker::DockerInterface;
use anyhow::{anyhow, Result};
use plane_core::{
    logging::LogError,
    messages::{
        agent::{BackendStatsMessage, DroneLogMessage},
        dns::{DnsRecordType, SetDnsRecord},
    },
    nats::TypedNats,
    types::{BackendId, ClusterName},
};
use std::{net::IpAddr, time::Duration};
use tokio::{task::JoinHandle, time::sleep};
use tokio_stream::StreamExt;

/// JoinHandle does not abort when it is dropped; this wrapper does.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct BackendMonitor {
    _log_loop: AbortOnDrop<Result<(), anyhow::Error>>,
    _stats_loop: AbortOnDrop<Result<(), anyhow::Error>>,
    _dns_loop: AbortOnDrop<Result<(), anyhow::Error>>,
}

impl BackendMonitor {
    pub fn new(
        backend_id: &BackendId,
        cluster: &ClusterName,
        ip: IpAddr,
        docker: &DockerInterface,
        nc: &TypedNats,
    ) -> Self {
        let log_loop = Self::log_loop(backend_id, docker, nc);
        let stats_loop = Self::stats_loop(backend_id, docker, nc);
        let dns_loop = Self::dns_loop(backend_id, ip, nc, cluster);

        BackendMonitor {
            _log_loop: AbortOnDrop(log_loop),
            _stats_loop: AbortOnDrop(stats_loop),
            _dns_loop: AbortOnDrop(dns_loop),
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
                nc.publish_jetstream(&SetDnsRecord {
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

    fn log_loop(
        backend_id: &BackendId,
        docker: &DockerInterface,
        nc: &TypedNats,
    ) -> JoinHandle<Result<(), anyhow::Error>> {
        let docker = docker.clone();
        let nc = nc.clone();
        let backend_id = backend_id.clone();

        tokio::spawn(async move {
            let container_name = backend_id.to_resource_name();
            tracing::info!(%backend_id, "Log recording loop started.");
            let mut stream = docker.get_logs(&container_name);

            while let Some(v) = stream.next().await {
                match v {
                    Ok(v) => {
                        if let Some(message) = DroneLogMessage::from_log_message(&backend_id, &v) {
                            nc.publish(&message).await?;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Error encountered forwarding log.");
                    }
                }
            }

            tracing::info!(%backend_id, "Log loop terminated.");

            Ok::<(), anyhow::Error>(())
        })
    }

    fn stats_loop(
        backend_id: &BackendId,
        docker: &DockerInterface,
        nc: &TypedNats,
    ) -> JoinHandle<Result<(), anyhow::Error>> {
        let docker = docker.clone();
        let nc = nc.clone();
        let backend_id = backend_id.clone();

        tokio::spawn(async move {
            let container_name = backend_id.to_resource_name();
            tracing::info!(%backend_id, "Stats recording loop started.");
            let mut stream = Box::pin(docker.get_stats(&backend_id));
            let mut prev_stats = stream
                .next()
                .await
                .ok_or_else(|| anyhow!("failed to get first stats"))??;
            while let Some(cur_stats) = stream.next().await {
                match cur_stats {
                    Ok(cur_stats) => {
                        nc.publish(&BackendStatsMessage::from_stats_messages(
                            &backend_id,
                            &prev_stats,
                            &cur_stats,
                        )?)
                        .await?;
                        prev_stats = cur_stats;
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Error encountered sending stats.")
                    }
                }
            }

            Ok(())
        })
    }
}
