use anyhow::{anyhow, Result};
use bat::PrettyPrinter;
use clap::{Parser, Subcommand};
use kube::{api::PostParams, Api, Client, CustomResourceExt};
use serde::Serialize;
use spawner_resource::{
    ImagePullPolicy, SessionLivedBackend, SessionLivedBackendBuilder, SPAWNER_GROUP,
};

const DEFAULT_NAMESPACE: &str = "default";

#[derive(Parser)]
struct Opts {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the custom resource definition (CRD) to stdout as YAML.
    PrintCrd,

    /// Print a spec for a SessionLivedBackend using the given image.
    PrintSlbe(SlbeSpec),

    /// Create a spec for a SessionLivedBackend using the given image.
    ///
    /// Equivalent to `spawner-cli print-slbe [options] | kubectl create -f -`
    CreateSlbe(SlbeSpec),
}

#[derive(Parser)]
struct SlbeSpec {
    /// The container registry ID of the docker container to run.
    #[clap(long)]
    image: String,

    /// The policy for when to pull a remote container image down.
    #[clap(long)]
    image_pull_policy: Option<ImagePullPolicy>,

    /// A unique identifier for the SessionLivedBackend; generated if omitted.
    #[clap(long)]
    name: Option<String>,

    /// A prefix to use if generating a unique identifier.
    #[clap(long)]
    prefix: Option<String>,

    /// The Kubernetes namespace to operate within.
    #[clap(long, default_value=DEFAULT_NAMESPACE)]
    namespace: String,

    #[clap(long, default_value="300")]
    grace_period_seconds: u32,
}

impl SlbeSpec {
    fn as_slbe(&self) -> SessionLivedBackend {
        let grace_period_seconds = if self.grace_period_seconds == 0 {
            None
        } else {
            Some(self.grace_period_seconds)
        };

        let builder = SessionLivedBackendBuilder::new(&self.image)
            .with_grace_period(grace_period_seconds)
            .with_image_pull_policy(self.image_pull_policy);

        if let Some(name) = &self.name {
            builder.build_named(name)
        } else if let Some(prefix) = &self.prefix {
            builder.build_prefixed(prefix)
        } else {
            builder.build()
        }
    }
}

fn pretty_print_yaml<T: Serialize>(obj: T) -> Result<()> {
    let s = serde_yaml::to_string(&obj)?;

    if atty::is(atty::Stream::Stdout) {
        PrettyPrinter::new()
            .input_from_bytes(s.as_bytes())
            .language("yaml")
            .print()?;
    } else {
        println!("{}", s);
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = Opts::parse();

    match opts.command {
        Command::PrintCrd => {
            pretty_print_yaml(&SessionLivedBackend::crd())?;
        }
        Command::PrintSlbe(spec) => {
            pretty_print_yaml(&spec.as_slbe())?;
        }
        Command::CreateSlbe(spec) => {
            let client = Client::try_default().await?;
            let api = Api::<SessionLivedBackend>::namespaced(client, &spec.namespace);
            let slbe = spec.as_slbe();

            let result = api
                .create(
                    &PostParams {
                        field_manager: Some(SPAWNER_GROUP.to_string()),
                        ..PostParams::default()
                    },
                    &slbe,
                )
                .await?;

            let name = result
                .metadata
                .name
                .ok_or_else(|| anyhow!("Expected created pod to have a name."))?;

            println!("Created: {:?}", name);
        }
    }

    Ok(())
}
