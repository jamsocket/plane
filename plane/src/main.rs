#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use plane::admin::AdminOpts;
use plane::client::PlaneClient;
use plane::controller::command::{controller_command, ControllerOpts};
use plane::database::connect_and_migrate;
use plane::dns::run_dns;
use plane::drone::command::DroneOpts;
use plane::drone::run_drone;
use plane::init_tracing::init_tracing;
use plane::names::{AcmeDnsServerName, OrRandom};
use plane::proxy::command::ProxyOpts;
use plane::proxy::run_proxy;
use plane::{PLANE_GIT_HASH, PLANE_VERSION};
use url::Url;

#[derive(Parser)]
struct Opts {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Controller(ControllerOpts),
    Drone(DroneOpts),
    Proxy(ProxyOpts),
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
        Command::Controller(opts) => controller_command(opts).await?,
        Command::Drone(opts) => run_drone(opts.into_config()?).await?,
        Command::Proxy(opts) => run_proxy(opts.into_config()?).await?,
        Command::Migrate { db } => {
            let _ = connect_and_migrate(&db).await?;
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
