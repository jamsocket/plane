use self::{
    agent::run_agent,
    cert::{refresh_certificate, refresh_loop, refresh_if_not_valid},
    cli::{DronePlan, Opts},
    proxy::serve,
};
use crate::{database::get_db, logging::{init_tracing, init_tracing_with_nats}};
use crate::{retry::do_with_retry};
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
    let opts = Opts::parse();
    let plan = DronePlan::try_from(opts)?;

    match plan {
        DronePlan::RunService {
            proxy_options,
            agent_options,
            cert_options,
            nats,
        } => {
            if let Some(nats) = nats {
                let nats = nats.connection().await?;
                init_tracing_with_nats(nats)?;
            } else {
                init_tracing()?;
            }

            let mut futs: Vec<Pin<Box<dyn Future<Output = Result<()>>>>> = vec![];

            if let Some(cert_options) = cert_options {
                do_with_retry(|| refresh_if_not_valid(&cert_options), 5, std::time::Duration::from_secs(10)).await?;

                futs.push(Box::pin(refresh_loop(cert_options)))
            }

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
