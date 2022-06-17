use self::{
    agent::run_agent,
    cert::{refresh_certificate, refresh_loop},
    cli::{DronePlan, Opts},
    proxy::serve,
};
use crate::{database::get_db, logging::init_tracing};
use anyhow::Result;
use clap::Parser;
use futures::{future::select_all, Future};
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::{pin::Pin, thread};

mod agent;
mod cert;
pub mod cli;
mod proxy;

async fn main() -> Result<()> {
    init_tracing()?; // TODO: init sender if we have nats.
    let opts = Opts::parse();
    let plan = DronePlan::try_from(opts)?;

    match plan {
        DronePlan::RunService {
            proxy_options,
            agent_options,
            cert_options,
        } => {
            let mut futs: Vec<Pin<Box<dyn Future<Output = Result<()>>>>> = vec![];

            if let Some(proxy_options) = proxy_options {
                futs.push(Box::pin(serve(proxy_options)));
            }

            if let Some(agent_options) = agent_options {
                futs.push(Box::pin(run_agent(agent_options)))
            }

            if let Some(cert_options) = cert_options {
                futs.push(Box::pin(refresh_loop(cert_options)))
            }

            let (result, _, _) = select_all(futs.into_iter()).await;
            result?;
        }
        DronePlan::DoMigration { db_path } => {
            get_db(&db_path).await?;
        }
        DronePlan::DoCertificateRefresh(cert_options) => {
            refresh_certificate(&cert_options).await?;
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
