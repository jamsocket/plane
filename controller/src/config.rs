use dis_spawner::nats_connection::NatsConnectionSpec;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct SchedulerOptions {}

#[derive(Serialize, Deserialize)]
pub struct DnsOptions {}

#[derive(Serialize, Deserialize)]
pub struct ControllerConfig {
    /// How to connect to NATS.
    pub nats: Option<NatsConnectionSpec>,

    pub scheduler: Option<SchedulerOptions>,

    pub dbs: Option<DnsOptions>,
}