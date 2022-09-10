use crate::config::DroneConfig;
use crate::{
    agent::run_agent,
    cert::{refresh_if_not_valid, refresh_loop},
    cli::DronePlan,
    proxy::serve,
};
use anyhow::Result;
use clap::Parser;
use dis_spawner::logging::TracingHandle;
use dis_spawner::retry::do_with_retry;
use futures::{future::select_all, Future};
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::{pin::Pin, thread};

#[derive(Parser)]
struct CliArgs {
    #[clap(short, long)]
    dump_config: bool,

    config_file: Option<String>,
}

async fn main() -> Result<()> {
    let mut tracing_handle = TracingHandle::init("drone".into())?;
    let cli_args = CliArgs::parse();

    let mut config_builder = config::Config::builder();
    if let Some(config_file) = cli_args.config_file {
        config_builder =
            config_builder.add_source(config::File::new(&config_file, config::FileFormat::Toml));
    }
    config_builder = config_builder.add_source(
        config::Environment::with_prefix("SPAWNER")
            .separator("__")
            .prefix_separator("_")
            .try_parsing(true)
            .list_separator(",")
            .with_list_parse_key("nats.hosts"),
    );
    let config: DroneConfig = config_builder.build()?.try_deserialize()?;

    if cli_args.dump_config {
        println!("{}", serde_json::to_string_pretty(&config)?);
        return Ok(());
    }

    let plan = DronePlan::from_drone_config(config).await?;

    let DronePlan {
        proxy_options,
        agent_options,
        cert_options,
        nats,
    } = plan;

    if let Some(nats) = nats {
        tracing_handle.attach_nats(nats)?;
    }

    let mut futs: Vec<Pin<Box<dyn Future<Output = Result<()>>>>> = vec![];

    if let Some(cert_options) = cert_options {
        do_with_retry(
            || refresh_if_not_valid(&cert_options),
            5,
            std::time::Duration::from_secs(10),
        )
        .await?;

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
