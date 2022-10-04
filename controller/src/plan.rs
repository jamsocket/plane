use crate::config::{ControllerConfig, DnsOptions};
use anyhow::Result;
use dis_spawner::nats::TypedNats;

pub struct SchedulerPlan;

pub struct DnsPlan {
    pub options: DnsOptions,
    pub nc: TypedNats,
}

pub struct ControllerPlan {
    pub nats: TypedNats,
    pub scheduler_plan: Option<SchedulerPlan>,
    pub dns_plan: Option<DnsPlan>,
}

impl ControllerPlan {
    pub async fn from_controller_config(config: ControllerConfig) -> Result<Self> {
        let nats = config.nats.connect_with_retry().await?;

        let scheduler_plan = config.scheduler.map(|_| SchedulerPlan);
        let dns_plan = config.dns.map(|options| DnsPlan {
            options: options,
            nc: nats.clone(),
        });

        Ok(ControllerPlan {
            nats,
            scheduler_plan,
            dns_plan,
        })
    }
}
