#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use plane::admin::AdminOpts;
use plane::client::PlaneClient;
use plane::controller::run_controller;
use plane::database::connect_and_migrate;
use plane::dns::run_dns;
use plane::drone::docker::PlaneDocker;
use plane::drone::run_drone;
use plane::init_tracing::init_tracing;
use plane::names::{AcmeDnsServerName, ControllerName, DroneName, Name, OrRandom, ProxyName};
use plane::proxy::{run_proxy, AcmeConfig, ServerPortConfig};
use plane::types::ClusterName;
use plane::{PLANE_GIT_HASH, PLANE_VERSION};
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

        #[clap(long, default_value = "127.0.0.1")]
        host: IpAddr,

        #[clap(long)]
        controller_url: Option<Url>,

        #[clap(long)]
        default_cluster: Option<ClusterName>,

        #[clap(long)]
        cleanup_min_age_days: Option<i32>,
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

        /// Optional log driver configuration, passed to Docker as the `LogConfig` field.
        #[clap(long)]
        log_config: Option<String>,
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

        /// URL to redirect the root path to.
        #[clap(long)]
        root_redirect_url: Option<Url>,
    },
    Dns {
        #[clap(long)]
        name: Option<AcmeDnsServerName>,

        #[clap(long)]
        controller_url: Url,

        /// Suffix to strip from requests before looking up TXT records.
        /// E.g. if the zone is "example.com", a TXT record lookup
        /// for foo.bar.baz.example.com
        /// will return the TXT records for the cluster "foo.bar.baz".
        ///
        /// The DNS record for _acme-challenge.foo.bar.baz in this case
        /// should have a CNAME record pointing to foo.bar.baz.example.com.
        #[clap(long)]
        zone: String,

        #[clap(long, default_value = "53")]
        port: u16,
    },
    Migrate {
        #[clap(long)]
        db: String,
    },
    Admin(AdminOpts),
    Version,
}

async fn run(opts: Opts) -> Result<()> {
    match opts.command {
        Command::Controller {
            host,
            port,
            db,
            controller_url,
            default_cluster,
            cleanup_min_age_days,
        } => {
            let name = ControllerName::new_random();

            let controller_url = match controller_url {
                Some(url) => url,
                None => Url::parse(&format!("http://{}:{}", host, port))?,
            };

            tracing::info!(%name, "Starting controller. Attempting to connect to database...");
            let db = connect_and_migrate(&db)
                .await
                .context("Failed to connect to database and run migrations.")?;
            tracing::info!("Connected to database.");

            let addr = (host, port).into();

            run_controller(
                db,
                addr,
                name,
                controller_url,
                default_cluster,
                cleanup_min_age_days,
            )
            .await?
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
            log_config,
        } => {
            let name = name.or_random();
            tracing::info!(%name, "Starting drone");

            let client = PlaneClient::new(controller_url);
            let docker = bollard::Docker::connect_with_local_defaults()?;

            let log_config = log_config.map(|s| serde_json::from_str(&s)).transpose()?;

            let docker = PlaneDocker::new(docker, docker_runtime, log_config).await?;

            run_drone(client, docker, name, cluster, ip, db.as_deref()).await?;
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
            root_redirect_url,
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
                    acme_eab_keypair: None,
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
                root_redirect_url,
            )
            .await?;
        }
        Command::Dns {
            name,
            controller_url,
            port,
            zone,
        } => {
            let name = name.or_random();
            tracing::info!("Starting DNS server");
            let client = PlaneClient::new(controller_url);
            run_dns(name, client, port, Some(zone)).await?;
        }
        Command::Admin(admin_opts) => {
            plane::admin::run_admin_command(admin_opts).await;
        }
        Command::Version => {
            println!("Client version: {}", PLANE_VERSION.bright_white());
            println!("Client hash: {}", PLANE_GIT_HASH.bright_white());
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
