#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use plane::admin::AdminOpts;
use plane::controller::command::ControllerOpts;
use plane::controller::run_controller;
use plane::database::connect_and_migrate;
use plane::dns::command::DnsOpts;
use plane::dns::run_dns;
use plane::drone::command::DroneOpts;
use plane::drone::run_drone;
use plane::init_tracing::init_tracing;
use plane::proxy::command::ProxyOpts;
use plane::proxy::run_proxy;
use plane::{Plan, PLANE_GIT_HASH, PLANE_VERSION};
use std::path::PathBuf;

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
    Dns(DnsOpts),
    /// Run a Plane instance from a JSON configuration file.
    Plan {
        config_file: PathBuf,
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
        Command::Controller(opts) => run_controller(opts.into_config()?).await?,
        Command::Drone(opts) => run_drone(opts.into_config()?).await?,
        Command::Proxy(opts) => run_proxy(opts.into_config()?).await?,
        Command::Migrate { db } => {
            let _ = connect_and_migrate(&db).await?;
        }
        Command::Dns(opts) => run_dns(opts.into_config()).await?,
        Command::Admin(admin_opts) => {
            plane::admin::run_admin_command(admin_opts).await;
        }
        Command::Version => {
            println!("Client version: {}", PLANE_VERSION.bright_white());
            println!("Client hash: {}", PLANE_GIT_HASH.bright_white());
        }
        Command::Plan { config_file } => {
            if !config_file.ends_with(".json") {
                // This check is so that we can potentially support other formats in the future
                // without breaking backwards compatibility.
                return Err(anyhow!("Config file must end in .json"));
            }

            let file = std::fs::File::open(config_file)?;
            let config = serde_json::from_reader(file)?;
            match config {
                Plan::Controller(config) => run_controller(config).await?,
                Plan::Dns(config) => run_dns(config).await?,
                Plan::Proxy(config) => run_proxy(config).await?,
                Plan::Drone(config) => run_drone(config).await?,
            }
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
