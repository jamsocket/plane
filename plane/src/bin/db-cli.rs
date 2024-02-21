use clap::{Parser, Subcommand};
use colored::{self, Colorize};
use plane::{
    database::connect,
    init_tracing::init_tracing,
    names::{BackendName, DroneName},
    types::ClusterName,
};

#[derive(Parser)]
struct Opts {
    #[clap(long)]
    db: String,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Events,
    ListNodes {
        #[clap(long)]
        all: bool,

        #[clap(long)]
        cluster: Option<ClusterName>,
    },
    ListBackends,
    TerminationCandidates {
        #[clap(long)]
        cluster: ClusterName,

        #[clap(long)]
        drone: DroneName,
    },
    PreventRenew {
        backend: BackendName,
    },
    Cleanup {
        /// The minimum amount of time that a data row can be obsolete before it is deleted.
        /// If not provided, only expired tokens will be removed, and other data will be retained.
        #[clap(long)]
        min_age_days: Option<i32>,
    },
}

async fn main_inner(opts: Opts) -> anyhow::Result<()> {
    let db = connect(&opts.db).await?;

    match opts.command {
        Command::PreventRenew { backend } => {
            let success = db.keys().prevent_renew(&backend).await?;

            if success {
                println!("Blocked renew for {}.", backend);
            } else {
                println!("Could not find active key for: {}", backend);
            }
        }
        Command::Events => {
            let mut events = db.subscribe_all_events();

            while let Ok(event) = events.recv().await {
                println!(
                    "{} {} {} {} {}",
                    event.timestamp.to_string().white(),
                    event
                        .id
                        .map(|id| id.to_string())
                        .unwrap_or("<None>".to_string())
                        .red(),
                    event.key.unwrap_or_else(|| "<global>".to_string()).yellow(),
                    event.kind.magenta(),
                    serde_json::to_string(&event.payload)?.blue()
                );
            }
        }
        Command::ListNodes { all, cluster } => {
            let nodes = db.node().list().await?;

            for node in nodes {
                if let Some(cluster) = &cluster {
                    if node.cluster.as_ref() != Some(cluster) {
                        continue;
                    }
                }

                if !all && !node.active() {
                    continue;
                }

                let connected_string = if let Some(controller) = &node.controller {
                    format!(
                        "{} to {}",
                        "Connected".green(),
                        controller.to_string().purple()
                    )
                } else {
                    "Disconnected".yellow().to_string()
                };

                if node.active() || all {
                    println!(
                        "{} {} {} Plane={}@{}",
                        connected_string,
                        node.cluster
                            .as_ref()
                            .map(|d| d.to_string().purple())
                            .unwrap_or_default(),
                        node.name.to_string().green(),
                        node.plane_version.yellow(),
                        node.plane_hash.yellow(),
                    );
                }
            }
        }
        Command::ListBackends => {
            let backends = db.backend().list_backends().await?;

            for backend in backends {
                println!(
                    "{} {} {} {} {}",
                    backend.id.to_string().blue(),
                    backend.cluster.green(),
                    backend.state.status().to_string().yellow(),
                    backend.last_status_time.to_string().white(),
                    backend.drone_id.to_string().green(),
                );
            }
        }
        Command::TerminationCandidates { cluster, drone } => {
            let drone_id = db.node().get_id(&cluster, &drone).await?;

            if let Some(drone_id) = drone_id {
                let backends = db.backend().termination_candidates(drone_id).await?;

                for termination_candidate in backends {
                    if let Some(expiration_time) = termination_candidate.expiration_time {
                        if expiration_time > termination_candidate.as_of {
                            println!(
                                "{} is alive past expiration time {}",
                                termination_candidate.backend_id.to_string().blue(),
                                expiration_time.to_string().white(),
                            );
                            continue;
                        }
                    }

                    if let Some(allowed_idle_seconds) = termination_candidate.allowed_idle_seconds {
                        let overage = termination_candidate.as_of
                            - termination_candidate.last_keepalive
                            - chrono::Duration::seconds(allowed_idle_seconds.into());
                        if overage > chrono::Duration::zero() {
                            println!(
                                "{} is alive past allowed {} seconds past idle time {}",
                                termination_candidate.backend_id.to_string().blue(),
                                overage.num_seconds().to_string().white(),
                                allowed_idle_seconds.to_string().white(),
                            );
                            continue;
                        }
                    }

                    println!(
                        "{} is a candidate for termination ({:?})",
                        termination_candidate.backend_id.to_string().blue(),
                        termination_candidate,
                    );
                }
            } else {
                println!("No such drone: {} on {}", drone, cluster);
            }
        }
        Command::Cleanup { min_age_days } => {
            plane::cleanup::run_cleanup(&db, min_age_days).await?;
        }
    };

    Ok(())
}

#[tokio::main]
async fn main() {
    init_tracing();

    let opts = Opts::parse();

    if let Err(e) = main_inner(opts).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
