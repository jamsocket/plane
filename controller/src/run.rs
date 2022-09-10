use crate::config::ControllerConfig;
use crate::plan::ControllerPlan;
use crate::run_scheduler;
use anyhow::{anyhow, Result};
use dis_spawner::{cli::init_cli, logging::TracingHandle, NeverResult};
use futures::future::try_join_all;
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::future::Future;
use std::pin::Pin;
use std::thread;

async fn controller_main() -> NeverResult {
    let mut tracing_handle = TracingHandle::init("controller".into())?;
    let config: ControllerConfig = init_cli()?;

    let plan = ControllerPlan::from_controller_config(config).await?;
    let ControllerPlan {
        nats,
        dns_plan,
        scheduler_plan,
    } = plan;

    tracing_handle.attach_nats(nats.clone())?;

    let mut futs: Vec<Pin<Box<dyn Future<Output = NeverResult>>>> = vec![];

    if let Some(_) = scheduler_plan {
        futs.push(Box::pin(run_scheduler(nats.clone())))
    }

    if let Some(_) = dns_plan {
        todo!("DNS server not let implemented.")
    }

    try_join_all(futs.into_iter()).await?;
    // try_join_all either returns an Err, or Ok() with a list of Never values.
    // Since Never values are not constructable, if we get here, we can assume that
    // try_join_all returned an empty list. This means that no event loops ever
    // actually ran, i.e. futs was empty.
    Err(anyhow!("No event loops selected."))
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
        .block_on(controller_main())?;

    Ok(())
}
