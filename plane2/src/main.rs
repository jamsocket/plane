use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use plane2::client::PlaneClient;
use plane2::controller::run_controller;
use plane2::database::connect_and_migrate;
use plane2::dns::run_dns;
use plane2::drone::run_drone;
use plane2::init_tracing::init_tracing;
use plane2::names::{AcmeDnsServerName, ControllerName, DroneName, Name, ProxyName};
use plane2::proxy::{run_proxy, AcmeConfig, ServerPortConfig};
use plane2::types::{ClusterName, OrRandom};
use std::net::IpAddr;
use std::path::PathBuf;
use url::Url;

const LOCAL_HTTP_PORT: u16 = 9090;
const PROD_HTTP_PORT: u16 = 80;
const PROD_HTTPS_PORT: u16 = 443;

#[derive(Parser)]
struct Opts {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Controller {
        #[clap(long)]
        db: String,

        #[clap(long, default_value = "8080")]
        port: u16,

        #[clap(long, default_value = "0.0.0.0")]
        host: IpAddr,
    },
    Drone {
        #[clap(long)]
        name: Option<DroneName>,

        #[clap(long)]
        controller_url: Url,

        #[clap(long)]
        cluster: ClusterName,

        /// IP address for this drone that proxies can connect to.
        #[clap(long, default_value = "127.0.0.1")]
        ip: IpAddr,

        /// Path to the database file. If omitted, an in-memory database will be used.
        #[clap(long)]
        db: Option<PathBuf>,

        #[clap(long)]
        docker_runtime: Option<String>,
    },
    Proxy {
        #[clap(long)]
        name: Option<ProxyName>,

        #[clap(long)]
        controller_url: Url,

        #[clap(long)]
        cluster: ClusterName,

        #[clap(long)]
        https: bool,

        #[clap(long)]
        http_port: Option<u16>,

        #[clap(long)]
        https_port: Option<u16>,

        #[clap(long)]
        cert_path: Option<PathBuf>,

        #[clap(long)]
        acme_endpoint: Option<Url>,

        #[clap(long)]
        acme_email: Option<String>,
    },
    Dns {
        #[clap(long)]
        name: Option<AcmeDnsServerName>,

        #[clap(long)]
        controller_url: Url,

        #[clap(long, default_value = "53")]
        port: u16,
    },
    Migrate {
        #[clap(long)]
        db: String,
    },
}

async fn run(opts: Opts) -> Result<()> {
    match opts.command {
        Command::Controller { host, port, db } => {
            let name = ControllerName::new_random();

            tracing::info!(%name, "Starting controller. Attempting to connect to database...");
            let db = connect_and_migrate(&db)
                .await
                .context("Failed to connect to database and run migrations.")?;
            tracing::info!("Connected to database.");

            let addr = (host, port).into();

            run_controller(db, addr, name).await?
        }
        Command::Migrate { db } => {
            let _ = connect_and_migrate(&db).await?;
        }
        Command::Drone {
            name,
            controller_url,
            cluster,
            ip,
            db,
            docker_runtime,
        } => {
            let name = name.or_random();
            tracing::info!(%name, "Starting drone");

            let client = PlaneClient::new(controller_url);
            let docker = bollard::Docker::connect_with_local_defaults()?;

            run_drone(
                client,
                docker,
                name,
                cluster,
                ip,
                db.as_deref(),
                docker_runtime,
            )
            .await?;
        }
        Command::Proxy {
            name,
            controller_url,
            cluster,
            https,
            http_port,
            https_port,
            cert_path,
            acme_endpoint,
            acme_email,
        } => {
            let name = name.or_random();
            tracing::info!(?name, "Starting proxy");
            let client = PlaneClient::new(controller_url);

            let port_config = match (https, http_port, https_port) {
                (false, None, None) => ServerPortConfig {
                    http_port: LOCAL_HTTP_PORT,
                    https_port: None,
                },
                (true, None, None) => ServerPortConfig {
                    http_port: PROD_HTTP_PORT,
                    https_port: Some(PROD_HTTPS_PORT),
                },
                (true, Some(http_port), None) => ServerPortConfig {
                    http_port,
                    https_port: Some(PROD_HTTPS_PORT),
                },
                (_, None, Some(https_port)) => ServerPortConfig {
                    http_port: PROD_HTTP_PORT,
                    https_port: Some(https_port),
                },
                (_, Some(http_port), https_port) => ServerPortConfig {
                    http_port,
                    https_port,
                },
            };

            let acme_config = match (acme_endpoint, acme_email) {
                (Some(_), None) => {
                    return Err(anyhow!(
                        "Must specify --acme-email when using --acme-endpoint."
                    ));
                }
                (None, Some(_)) => {
                    return Err(anyhow!(
                        "Must specify --acme-endpoint when using --acme-email."
                    ));
                }
                (Some(endpoint), Some(email)) => Some(AcmeConfig {
                    endpoint,
                    mailto_email: email,
                    client: reqwest::Client::new(),
                }),
                (None, None) => None,
            };

            run_proxy(
                name,
                client,
                cluster,
                cert_path.as_deref(),
                port_config,
                acme_config,
            )
            .await
            .unwrap();
        }
        Command::Dns {
            name,
            controller_url,
            port,
        } => {
            let name = name.or_random();
            tracing::info!("Starting DNS server");
            let client = PlaneClient::new(controller_url);
            run_dns(name, client, port).await.unwrap();
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let opts = Opts::parse();

    init_tracing();

    let result = run(opts).await;
    match result {
        Ok(()) => {}
        Err(message) => {
            tracing::error!(?message, "Error running command.");
        }
    }
}
