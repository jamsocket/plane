use super::{types::ContainerId, MetricsCallback};
use bollard::{container::StatsOptions, Docker};
use futures_util::Stream;
use plane_common::{names::BackendName, protocol::BackendMetricsMessage};
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, Instant};
use tokio_stream::StreamExt;

const METRICS_INTERVAL: Duration = Duration::from_secs(10);

fn stream_metrics(
    docker: &Docker,
    container_id: &ContainerId,
) -> impl Stream<Item = Result<bollard::container::Stats, bollard::errors::Error>> {
    let options = StatsOptions {
        stream: true,
        one_shot: false,
    };

    docker.stats(container_id.as_str(), Some(options))
}

pub async fn metrics_loop(
    backend_id: BackendName,
    docker: Docker,
    callback: Arc<Mutex<Option<MetricsCallback>>>,
) {
    let container_id = ContainerId::from(&backend_id);
    let mut stream = stream_metrics(&docker, &container_id);
    let mut last_processed = Instant::now() - METRICS_INTERVAL;

    while let Some(stats) = stream.next().await {
        // These metrics come from Docker every second, so let's throttle them to be less frequent.
        let now = Instant::now();
        if now.duration_since(last_processed) < METRICS_INTERVAL {
            continue;
        }
        last_processed = now;

        let stats = match stats {
            Err(err) => {
                tracing::error!(?err, "Error getting metrics for {container_id}");
                break;
            }
            Ok(stats) => stats,
        };

        if stats.memory_stats.stats.is_none() {
            // If the stats come back empty, it could be because the container is not running, in which case
            // we can close the metrics loop and stop the container.
            let container_info = match docker.inspect_container(container_id.as_str(), None).await {
                Err(err) => {
                    tracing::error!(?err, "Error getting container info for {container_id}");
                    break;
                }
                Ok(container_info) => container_info,
            };

            let running = container_info
                .state
                .as_ref()
                .and_then(|state| state.running)
                .unwrap_or(false);
            if !running {
                tracing::info!("Container {container_id} is not running, stopping metrics.");
                break;
            }
        }

        let callback = callback.lock().expect("Metrics callback lock poisoned");
        if let Some(callback) = callback.as_ref() {
            match metrics_message_from_container_stats(stats, backend_id.clone()) {
                Ok(Some(metrics_message)) => {
                    (callback)(metrics_message);
                }
                Ok(None) => (),
                Err(err) => {
                    tracing::error!(?err, "Error converting metrics for {container_id}");
                }
            }
        }
    }
}

fn metrics_message_from_container_stats(
    stats: bollard::container::Stats,
    backend_id: BackendName,
) -> anyhow::Result<Option<BackendMetricsMessage>> {
    let mem_stats = stats
        .memory_stats
        .stats
        .ok_or_else(|| anyhow::anyhow!("No memory stats found in stats."))?;
    let mem_used_total_docker = stats
        .memory_stats
        .usage
        .ok_or_else(|| anyhow::anyhow!("No memory usage found in stats."))?;
    let mem_limit = stats
        .memory_stats
        .limit
        .ok_or_else(|| anyhow::anyhow!("No memory limit found in stats."))?;

    let Some(total_system_cpu_used) = stats.cpu_stats.system_cpu_usage else {
        tracing::debug!("No system cpu usage found in stats (normal on first stats event).");
        return Ok(None);
    };
    let Some(prev_total_system_cpu_used) = stats.precpu_stats.system_cpu_usage else {
        tracing::debug!(
            "No previous system cpu usage found in stats (normal on first stats event)."
        );
        return Ok(None);
    };

    let container_cpu_used = stats.cpu_stats.cpu_usage.total_usage;
    let prev_container_cpu_used = stats.precpu_stats.cpu_usage.total_usage;

    if container_cpu_used < prev_container_cpu_used {
        return Err(anyhow::anyhow!(
            "Container cpu usage is less than previous total cpu usage."
        ));
    }
    if total_system_cpu_used < prev_total_system_cpu_used {
        return Err(anyhow::anyhow!(
            "Total system cpu usage is less than previous total system cpu usage."
        ));
    }

    let container_cpu_used_delta = container_cpu_used - prev_container_cpu_used;
    let system_cpu_used_delta = total_system_cpu_used - prev_total_system_cpu_used;

    let (mem_total, mem_active, mem_inactive, mem_unevictable, mem_used) = match mem_stats {
        bollard::container::MemoryStatsStats::V1(v1_stats) => {
            let active_mem = v1_stats.total_active_anon + v1_stats.total_active_file;
            let total_mem = v1_stats.total_rss + v1_stats.total_cache;
            let unevictable_mem = v1_stats.total_unevictable;
            let inactive_mem = v1_stats.total_inactive_anon + v1_stats.total_inactive_file;
            let mem_used = mem_used_total_docker - v1_stats.total_inactive_file;
            (
                total_mem,
                active_mem,
                inactive_mem,
                unevictable_mem,
                mem_used,
            )
        }
        bollard::container::MemoryStatsStats::V2(v2_stats) => {
            let active_mem = v2_stats.active_anon + v2_stats.active_file;
            let kernel = v2_stats.kernel_stack + v2_stats.sock + v2_stats.slab;
            let total_mem = v2_stats.file + v2_stats.anon + kernel;
            let unevictable_mem = v2_stats.unevictable;
            let inactive_mem = v2_stats.inactive_anon + v2_stats.inactive_file;
            let mem_used = mem_used_total_docker - v2_stats.inactive_file;
            (
                total_mem,
                active_mem,
                inactive_mem,
                unevictable_mem,
                mem_used,
            )
        }
    };

    Ok(Some(BackendMetricsMessage {
        backend_id,
        mem_total,
        mem_used,
        mem_active,
        mem_inactive,
        mem_unevictable,
        mem_limit,
        cpu_used: container_cpu_used_delta,
        sys_cpu: system_cpu_used_delta,
    }))
}
