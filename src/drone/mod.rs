use self::cli::{DronePlan, Opts};
use crate::{
    agent::run_agent, cert::refresh_certificate, database::get_db, logging::init_tracing,
    proxy::serve,
};
use anyhow::Result;
use clap::Parser;
use futures::{future::select_all, Future};
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::{pin::Pin, thread};

pub mod cli;

async fn main() -> Result<()> {
    init_tracing()?; // TODO: init sender if we have nats.
    let opts = Opts::parse();
    let plan = DronePlan::try_from(opts)?;

    match plan {
        DronePlan::RunService {
            proxy_options,
            agent_options,
        } => {
            let mut futs: Vec<Pin<Box<dyn Future<Output = Result<()>>>>> = vec![];

            if let Some(proxy_options) = proxy_options {
                futs.push(Box::pin(serve(proxy_options)));
            }

            if let Some(agent_options) = agent_options {
                futs.push(Box::pin(run_agent(agent_options)))
            }

            let (result, _, _) = select_all(futs.into_iter()).await;
            result?;
        }
        DronePlan::DoMigration { db_path } => {
            get_db(&db_path).await?;
        }
        DronePlan::DoCertificateRefresh {
            acme_server_url,
            cluster_domain,
            key_paths,
            nats_url,
        } => {
            refresh_certificate(&cluster_domain, &nats_url, &key_paths, &acme_server_url).await?;
        }
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let mut signals = Signals::new(&[SIGINT])?;

    thread::spawn(move || {
        for _ in signals.forever() {
            // TODO: we could shut down containers here.
            std::process::exit(0)
        }
    });

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(main())?;

    Ok(())
}
