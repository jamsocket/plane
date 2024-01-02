use clap::{Parser, Subcommand};
use colored::Colorize;
use plane::{
    client::{PlaneClient, PlaneClientError},
    init_tracing::init_tracing,
    names::{BackendName, DroneName},
    types::{BackendStatus, ClusterName, ConnectRequest, ExecutorConfig, KeyConfig, SpawnConfig},
};
use url::Url;

fn show_error(error: &PlaneClientError) {
    match error {
        PlaneClientError::Http(error) => {
            eprintln!(
                "{}: {}",
                "HTTP error from API server".bright_red(),
                error.to_string().magenta()
            );
        }
        PlaneClientError::Url(error) => {
            eprintln!(
                "{}: {}",
                "URL error".bright_red(),
                error.to_string().magenta()
            );
        }
        PlaneClientError::Json(error) => {
            eprintln!(
                "{}: {}",
                "JSON error".bright_red(),
                error.to_string().magenta()
            );
        }
        PlaneClientError::UnexpectedStatus(status) => {
            eprintln!(
                "{}: {}",
                "Unexpected status code from API server".bright_red(),
                status.to_string().magenta()
            );
        }
        PlaneClientError::PlaneError(error, status) => {
            eprintln!(
                "{}:\n    Message: {}\n    HTTP status: {}\n    Error ID for correlating with logs: {}",
                "API returned error status".bright_red(),
                error.message.magenta().bold(),
                status.to_string().bright_cyan(),
                error.id.bright_yellow(),
            );
        }
    }
}

#[derive(Parser)]
struct Opts {
    #[clap(long)]
    controller: Url,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Connect {
        #[clap(long)]
        cluster: ClusterName,

        #[clap(long)]
        image: String,

        #[clap(long)]
        key: Option<String>,

        #[clap(long)]
        wait: bool,
    },
    Terminate {
        #[clap(long)]
        cluster: ClusterName,

        #[clap(long)]
        backend: BackendName,

        #[clap(long)]
        hard: bool,

        #[clap(long)]
        wait: bool,
    },
    Drain {
        #[clap(long)]
        cluster: ClusterName,

        #[clap(long)]
        drone: DroneName,
    },
}

async fn inner_main(opts: Opts) -> Result<(), PlaneClientError> {
    let client = PlaneClient::new(opts.controller);

    match opts.command {
        Command::Connect {
            cluster,
            image,
            key,
            wait,
        } => {
            let executor_config = ExecutorConfig::from_image_with_defaults(image);
            let spawn_config = SpawnConfig {
                executable: executor_config.clone(),
                lifetime_limit_seconds: None,
                max_idle_seconds: Some(500),
            };
            let key_config = key.map(|name| KeyConfig {
                name,
                ..Default::default()
            });
            let spawn_request = ConnectRequest {
                spawn_config: Some(spawn_config),
                key: key_config,
                ..Default::default()
            };

            let response = client.connect(&cluster, &spawn_request).await?;

            println!(
                "Created backend: {}",
                response.backend_id.to_string().bright_green()
            );

            println!("URL: {}", response.url.bright_white());
            println!("Status URL: {}", response.status_url.bright_white());

            if wait {
                let mut stream = client
                    .backend_status_stream(&cluster, &response.backend_id)
                    .await?;

                while let Some(status) = stream.next().await {
                    println!(
                        "Status: {} at {}",
                        status.status.to_string().magenta(),
                        status.time.to_string().bright_cyan()
                    );

                    if status.status >= BackendStatus::Ready {
                        break;
                    }
                }
            }
        }
        Command::Terminate {
            backend,
            cluster,
            hard,
            wait,
        } => {
            if hard {
                client.hard_terminate(&cluster, &backend).await?
            } else {
                client.soft_terminate(&cluster, &backend).await?
            };

            println!(
                "Sent termination signal {}",
                backend.to_string().bright_green()
            );

            if wait {
                let mut stream = client.backend_status_stream(&cluster, &backend).await?;

                while let Some(status) = stream.next().await {
                    println!(
                        "Status: {} at {}",
                        status.status.to_string().magenta(),
                        status.time.to_string().bright_cyan()
                    );

                    if status.status >= BackendStatus::Terminated {
                        break;
                    }
                }
            }
        }
        Command::Drain { cluster, drone } => {
            client.drain(&cluster, &drone).await?;
            println!("Sent drain signal to {}", drone.to_string().bright_green());
        }
    };

    Ok(())
}

#[tokio::main]
async fn main() {
    let opts = Opts::parse();
    init_tracing();

    let result = inner_main(opts).await;

    if let Err(error) = result {
        show_error(&error);
        std::process::exit(1);
    }
}
