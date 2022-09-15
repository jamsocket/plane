use dis_spawner::nats_connection::NatsConnectionSpec;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct SchedulerOptions {}

#[derive(Serialize, Deserialize)]
pub struct DnsOptions {}

#[derive(Serialize, Deserialize)]
pub struct ControllerConfig {
    /// How to connect to NATS.
    pub nats: NatsConnectionSpec,

    pub scheduler: Option<SchedulerOptions>,

    pub dns: Option<DnsOptions>,
}
