use super::record_stats::Stats;
use super::ErrorObj;
use nats;
use serde::Serialize;
use std::boxed::Box;
use std::error::Error;
use std::str::FromStr;
pub struct NatsSubjectComponent(pub String);

#[derive(Serialize)]
pub struct DroneStatsMessage {
    #[serde(flatten)]
    pub stats: Stats,
    pub cluster: String,
    pub drone: String,
}

impl FromStr for NatsSubjectComponent {
    type Err = Box<dyn Error>;
    fn from_str(a: &str) -> Result<Self, Self::Err> {
        if a.contains(char::is_whitespace) {
            return Err("provided nats subject components contain whitespace!".into());
        }
        Ok(NatsSubjectComponent(a.replace(".","_").into()))
    }
}

pub type Sender = Box<dyn Fn(&str) -> Result<(), ErrorObj>>;
pub fn get_nats_sender(nats_url: &str, subject: &str) -> Result<Sender, ErrorObj> {
    let nc = nats::connect(nats_url)?;
    let sub_clone = subject.to_owned();

    Ok(Box::new(move |msg: &str| {
        nc.publish(&sub_clone, msg)?;
        Ok(())
    }))
}
