use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use plane_core::{
    messages::{
        agent::{
            BackendState, BackendStateMessage, DockerExecutableConfig, ResourceLimits,
            TerminationRequest,
        },
        scheduler::{DrainDrone, ScheduleRequest, ScheduleResponse},
        state::{
            BackendMessage, BackendMessageType, ClusterStateMessage, DroneMessage,
            DroneMessageType, WorldStateMessage,
        },
    },
    nats_connection::NatsConnectionSpec,
    state::get_world_state,
    types::{BackendId, ClusterName, DroneId},
};
use std::{collections::HashMap, time::Duration};
use time::macros::format_description;

#[derive(Parser)]
struct Opts {
    #[clap(long)]
    nats: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    ListDrones,
    ListBackends,
    Spawn {
        cluster: String,
        image: String,
        /// Grace period with no connections before shutting down the drone.
        #[clap(long, default_value = "300")]
        timeout: u64,
        #[clap(long, short)]
        port: Option<u16>,
        #[clap(long, short)]
        env: Vec<String>,
        /// Optional string to use as a lock. If another backend with the same
        /// lock string is already running, the spawn will return that backend
        /// instead of spawning a new one.
        #[clap(long)]
        lock: Option<String>,
    },
    Status {
        backend: Option<String>,
    },
    Drain {
        cluster: String,

        #[arg(required = true)]
        drone: Vec<String>,

        /// Cancel draining and allow a drone to accept backends again.
        #[clap(long)]
        cancel: bool,
    },
    Terminate {
        cluster: String,
        backend: String,
    },
    DumpState,
    StreamState {
        /// Whether to include heartbeat messages.
        #[clap(long, default_value = "false")]
        include_heartbeat: bool,

        /// Stream until we receive a snapshot, then exit.
        #[clap(long, default_value = "false")]
        snapshot: bool,
    },
    /// Remove swept backends.
    Cleanup {
        /// Whether to include backends with no state information.
        /// Note that this will remove backends that have just been created
        /// and don't have any state information yet.
        #[clap(long, default_value = "false")]
        include_missing_state: bool,

        /// Whether to do a dry run.
        #[clap(long, default_value = "false")]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    tracing_subscriber::fmt().init();

    let nats = NatsConnectionSpec::from_url(opts.nats.as_deref().unwrap_or("nats://localhost"))?
        .connect("cli.inbox")
        .await?;

    match opts.command {
        Command::Cleanup {
            dry_run,
            include_missing_state,
        } => {
            let state = get_world_state(nats.clone()).await?;
            let stream = nats.jetstream.get_stream("plane_state").await.unwrap();

            for (cluster_name, cluster) in &state.clusters {
                for (backend_id, backend) in &cluster.backends {
                    if let Some((timestamp, state)) = backend.state_timestamp() {
                        if state.terminal() {
                            println!(
                                "Removing backend {} from cluster {}, {} at {}",
                                backend_id, cluster_name, state, timestamp
                            );
                        } else if timestamp < chrono::Utc::now() - chrono::Duration::hours(24)
                            && (state != BackendState::Ready)
                        {
                            // This catches backends that remained in a non-ready and non-terminal state (Loading or Starting)
                            // for more than 24 hours. This either means we lost the drone, or the drone has a bug.
                            println!(
                                "Removing abandoned backend {} from cluster {}, last update was {} at {}",
                                backend_id, cluster_name, state, timestamp
                            );
                        } else {
                            // Ignore this backend.
                            continue;
                        }
                    } else if include_missing_state {
                        println!(
                            "Removing backend {} from cluster {}, no state information",
                            backend_id, cluster_name
                        );
                    } else {
                        continue;
                    };

                    let subject = format!(
                        "state.cluster.{}.backend.{}",
                        cluster_name.subject_name(),
                        backend_id.id()
                    );
                    // NB: this is only needed because assignments are stored directly
                    // at state.cluster.{}.backend.{}.
                    let subjects = [subject.clone(), format!("{}.>", subject)];

                    if dry_run {
                        println!("Would purge {:?}", subjects);
                    } else {
                        println!("Purging {:?}", subjects);

                        for subject in subjects.iter() {
                            stream.purge().filter(subject).await.unwrap();
                        }

                        // TEMP: sleep a bit to avoid stressing NATS until
                        // we have a better grasp of performance implications.
                        // We probably don't need this, but it's cheap.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
        Command::DumpState => {
            let state = get_world_state(nats.clone()).await?;
            println!("{:#?}", state);
        }
        Command::StreamState {
            include_heartbeat,
            snapshot,
        } => {
            let mut sub = nats
                .subscribe_jetstream_subject(WorldStateMessage::subscribe_subject())
                .await?;

            while let Some((message, meta)) = sub.next().await {
                if snapshot && !sub.has_pending() {
                    break;
                }

                let WorldStateMessage::ClusterMessage { cluster, message } = message else {
                    // ignore heartbeats
                    continue;
                };

                let text = match message {
                    ClusterStateMessage::AcmeMessage(acme) => {
                        format!("ACME TXT entry: {}", acme.value.to_string().bright_blue())
                    }
                    ClusterStateMessage::LockMessage(lock) => {
                        let lock_name = format!("{}", lock.lock);
                        let uid = format!("{:x}", lock.uid);
                        let lock_message = format!("{:?}", lock.message);
                        format!(
                            "Lock: {} (uid: {}) {}",
                            lock_name.bright_cyan(),
                            uid.bright_yellow(),
                            lock_message.bright_magenta()
                        )
                    }
                    ClusterStateMessage::DroneMessage(drone) => {
                        let DroneMessage { drone, message } = drone;

                        let text = match message {
                            DroneMessageType::KeepAlive { timestamp } => {
                                if !include_heartbeat {
                                    continue;
                                }
                                format!("is alive at {}", timestamp.to_string().bright_blue())
                            }
                            DroneMessageType::Metadata(metadata) => {
                                format!(
                                    "has IP: {}, version: {}, git hash: {}",
                                    metadata.ip.to_string().bright_blue(),
                                    metadata.version.to_string().bright_blue(),
                                    metadata
                                        .git_hash_short()
                                        .unwrap_or("unknown".to_string())
                                        .bright_blue()
                                )
                            }
                            DroneMessageType::State { state, timestamp } => {
                                format!(
                                    "is {} at {}",
                                    state.to_string().bright_magenta(),
                                    timestamp.to_string().bright_yellow()
                                )
                            }
                        };

                        format!("Drone: {} {}", drone.to_string().bright_yellow(), text)
                    }
                    ClusterStateMessage::BackendMessage(backend) => {
                        let BackendMessage { backend, message } = backend;

                        let text = match message {
                            BackendMessageType::State { state, timestamp } => {
                                format!(
                                    "is {} at {}",
                                    state.to_string().bright_magenta(),
                                    timestamp.to_string().bright_yellow()
                                )
                            }
                            BackendMessageType::Assignment {
                                drone,
                                lock_assignment,
                                ..
                            } => {
                                let mut text = format!(
                                    "is assigned to drone {}",
                                    drone.to_string().bright_yellow()
                                );

                                if let Some(lock) = lock_assignment {
                                    let lock_name = format!("{}", lock.lock);
                                    let uid = format!("{:x}", lock.uid);
                                    text.push_str(&format!(
                                        " with lock {} (uid: {})",
                                        lock_name.bright_cyan(),
                                        uid.bright_yellow()
                                    ));
                                }

                                text
                            }
                        };
                        format!("Backend: {} {}", backend.to_string().bright_cyan(), text)
                    }
                };

                let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
                let timestamp = meta.timestamp.format(format)?.to_string();
                println!(
                    "{} cluster: {} {}",
                    timestamp.bright_purple(),
                    cluster.to_string().bright_green(),
                    text
                );
            }
        }
        Command::Status { backend } => {
            let mut sub = if let Some(backend) = backend {
                nats.subscribe_jetstream_subject(BackendStateMessage::subscribe_subject(
                    &BackendId::new(backend),
                ))
                .await?
            } else {
                nats.subscribe_jetstream().await?
            };

            while let Some((message, _)) = sub.next().await {
                println!(
                    "{}\t{}\t{}",
                    message.backend.to_string().bright_cyan(),
                    message.state.to_string().bright_magenta(),
                    message.time.to_string().blue()
                );
            }
        }
        Command::ListDrones => {
            let state = get_world_state(nats.clone()).await?;
            for (cluster_name, cluster) in &state.clusters {
                println!("{}", cluster_name.to_string().bright_green());
                for (drone_id, drone) in &cluster.drones {
                    println!(
                        "\t{}\t{}\t{}\t{}\t{}\t{}",
                        drone_id.to_string().bright_blue(),
                        drone
                            .state()
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                            .bright_magenta(),
                        drone
                            .last_seen
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                            .blue(),
                        drone
                            .meta
                            .as_ref()
                            .map(|d| d.ip.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                            .bright_yellow(),
                        drone
                            .meta
                            .as_ref()
                            .map(|d| d.version.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                            .bright_cyan(),
                        drone
                            .meta
                            .as_ref()
                            .and_then(|d| d.git_hash_short())
                            .unwrap_or_else(|| "unknown".to_string())
                            .bright_purple(),
                    );
                }
            }
        }
        Command::Spawn {
            image,
            cluster,
            timeout,
            port,
            env,
            lock,
        } => {
            let env: Result<HashMap<String, String>> = env
                .iter()
                .map(|d| {
                    let (key, value) = d
                        .split_once('=')
                        .context("Expected environment variables in the form KEY=VALUE")?;

                    Ok((key.to_string(), value.to_string()))
                })
                .collect();
            let env = env?;

            let result = nats
                .request(&ScheduleRequest {
                    backend_id: None,
                    cluster: ClusterName::new(&cluster),
                    max_idle_secs: Duration::from_secs(timeout),
                    metadata: HashMap::new(),
                    executable: DockerExecutableConfig {
                        image,
                        env,
                        credentials: None,
                        resource_limits: ResourceLimits::default(),
                        pull_policy: Default::default(),
                        port,
                        volume_mounts: Vec::new(),
                    },
                    require_bearer_token: false,
                    lock: lock.map(|lock| lock.try_into().unwrap()),
                })
                .await?;

            match result {
                ScheduleResponse::Scheduled {
                    drone,
                    backend_id,
                    bearer_token,
                    spawned,
                } => {
                    let url = format!("https://{}.{}", backend_id, cluster);

                    if spawned {
                        println!("Backend scheduled.");
                    } else {
                        println!("A backend already held the desired lock.");
                    }

                    println!("URL: {}", url.bright_green());
                    println!("Drone: {}", drone.to_string().bright_blue());
                    println!("Backend ID: {}", backend_id.to_string().bright_blue());
                    if let Some(bearer_token) = bearer_token {
                        println!("Bearer token: {}", bearer_token.bright_blue());
                    }
                }
                ScheduleResponse::NoDroneAvailable => tracing::error!(
                    %cluster,
                    "Could not schedule backend because no drone was available for cluster."
                ),
            }
        }
        Command::ListBackends => {
            let state = get_world_state(nats.clone()).await?;
            for (cluster_name, cluster) in &state.clusters {
                println!("{}", cluster_name.to_string().bright_green());
                for (backend_id, backend) in &cluster.backends {
                    println!(
                        "\t{}.{}\t{}",
                        backend_id.to_string().bright_blue(),
                        cluster_name.to_string().bright_green(),
                        backend
                            .state()
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                            .bright_magenta(),
                    );
                }
            }
        }
        Command::Terminate {
            cluster, backend, ..
        } => {
            nats.request(&TerminationRequest {
                backend_id: BackendId::new(backend),
                cluster_id: ClusterName::new(&cluster),
            })
            .await?;

            println!("{}", "Terminated successfully".bright_green());
        }
        Command::Drain {
            drone,
            cluster,
            cancel,
        } => {
            let drain = !cancel;
            for ref drone in drone {
                match nats
                    .request(&DrainDrone {
                        cluster: ClusterName::new(&cluster),
                        drone: DroneId::new(drone.clone()),
                        drain,
                    })
                    .await
                {
                    Ok(_) => {
                        if drain {
                            println!("{} {}", "Draining started on drone: ".bright_green(), drone);
                        } else {
                            println!(
                                "{} {}",
                                "Draining cancelled on drone: ".bright_green(),
                                drone
                            );
                        }
                    }
                    Err(e) => {
                        println!("{} {}", "Draining failed on drone: ".bright_red(), drone);
                        println!("Error: {}", e.to_string().bright_red())
                    }
                }
            }
        }
    }

    Ok(())
}
