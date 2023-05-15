pub mod record_stats;
pub mod utils;

use getopts::{Matches, Options};
use record_stats::{get_stats_recorder, Stats};
use std::env;
use std::error::Error;
use std::{thread, time};
use utils::{get_nats_sender, DroneStatsMessage, NatsSubjectComponent};

pub type ErrorObj = Box<dyn Error>;

const REPORTING_INTERVAL_MS: u64 = 1000;

enum CliOptions {
    WithNats {
        nats_url: String,
        drone_id: NatsSubjectComponent,
        cluster_name: NatsSubjectComponent,
    },
    Test,
}

fn parse_args() -> Result<CliOptions, ErrorObj> {
    let args: Vec<String> = env::args().collect();
    let mut options = Options::new();
    options.optopt(
        "n",
        "nats_url",
        "set nats url",
        "nats url of the form nats://<AUTH>@<HOST_URL>[:<PORT>]",
    );
    options.optopt(
        "c",
        "cluster_name",
        "plane cluster name",
        "note: no whitespace or periods permitted",
    );
    options.optopt(
        "d",
        "drone_id",
        "drone id",
        "note: no whitespace or periods permitted",
    );
    options.optflag(
        "t",
        "test",
        "enables test mode, writing to stdout instead of nats",
    );
    options.optflag("h", "help", "print this help menu");

    let matches = options.parse(&args[1..])?;

    handle_args(matches).map_err(|e| {
        eprint!("{}", options.usage(""));
        e
    })
}

fn handle_args(matches: Matches) -> Result<CliOptions, ErrorObj> {
    let help = matches.opt_present("h");
    let test = matches.opt_present("t");
    let nats_url = matches.opt_get::<String>("nats_url")?;
    let drone_id = matches.opt_get::<NatsSubjectComponent>("drone_id")?;
    let cluster_name = matches.opt_get::<NatsSubjectComponent>("cluster_name")?;
    if help {
        return Err("".into());
    }

    match test {
        false => Ok(CliOptions::WithNats {
            nats_url: nats_url.ok_or("nats url undefined!")?,
            drone_id: drone_id.ok_or("drone id undefined!")?,
            cluster_name: cluster_name.ok_or("cluster name undefined!")?,
        }),
        true => Ok(CliOptions::Test),
    }
}

fn main() -> Result<(), ErrorObj> {
    let cli_opts = parse_args()?;
    let (cluster, drone, send): (_, _, Box<dyn Fn(&str) -> Result<(), ErrorObj>>) = match cli_opts {
        CliOptions::Test => {
            let dummysend = Box::new(|msg: &str| -> Result<(), ErrorObj> {
                println!("{msg}");
                Ok(())
            });
            let cluster: String = "DUMMY_CLUSTER_NAME".into();
            let drone: String = "DUMMY_DRONE_ID".into();
            (cluster, drone, dummysend)
        }
        CliOptions::WithNats {
            drone_id,
            cluster_name,
            nats_url,
        } => {
            let nats_subject = format!("cluster.{}.drone.{}.stats", cluster_name.0, drone_id.0);
            let send = get_nats_sender(&nats_url, &nats_subject)?;
            (cluster_name.0, drone_id.0, send)
        }
    };
    let mut record_stats = get_stats_recorder();

    let append_stats = |stats: Stats| DroneStatsMessage {
        cluster: cluster.clone(),
        drone: drone.clone(),
        stats,
    };

    loop {
        let stats = record_stats();
        let to_send = append_stats(stats);
        let stats_json = serde_json::to_string(&to_send)?;
        send(&stats_json)?;
        thread::sleep(time::Duration::from_millis(REPORTING_INTERVAL_MS));
    }
}
