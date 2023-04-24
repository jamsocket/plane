use anyhow::Result;
use clap::Parser;
use serde::{de::DeserializeOwned, Serialize};
use tokio::signal::unix::SignalKind;

#[derive(Parser)]
struct CliArgs {
    #[clap(short, long)]
    dump_config: bool,

    config_file: Option<String>,
}

pub async fn wait_for_interrupt() -> Result<()> {
    let mut int_stream = tokio::signal::unix::signal(SignalKind::interrupt())?;
    let mut term_stream = tokio::signal::unix::signal(SignalKind::terminate())?;
    tokio::select!(
        _ = int_stream.recv() => {
            tracing::info!("Received SIGINT, exiting.");
        },
        _ = term_stream.recv() => {
            tracing::info!("Received SIGTERM, exiting.");
        },
    );

    Ok(())
}

pub fn init_cli<C: Serialize + DeserializeOwned>() -> Result<C> {
    let cli_args = CliArgs::parse();

    let mut config_builder = config::Config::builder();
    if let Some(config_file) = cli_args.config_file {
        config_builder =
            config_builder.add_source(config::File::new(&config_file, config::FileFormat::Toml));
    }
    config_builder = config_builder.add_source(
        config::Environment::with_prefix("PLANE")
            .separator("__")
            .prefix_separator("_")
            .try_parsing(true)
            .list_separator(",")
            .with_list_parse_key("nats.hosts"),
    );
    let config: C = config_builder.build()?.try_deserialize()?;

    if cli_args.dump_config {
        println!("{}", serde_json::to_string_pretty(&config)?);
        // return Ok(());
        todo!("exit gracefully");
    }

    Ok(config)
}
