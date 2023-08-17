use crate::{
    config::{ControllerConfig, HttpOptions},
    dns::rname_format::format_rname,
};
use anyhow::{Context, Result};
use plane_core::{
    nats::TypedNats,
    state::{start_state_loop, StateHandle},
};
use std::net::IpAddr;
use trust_dns_server::client::rr::Name;

pub struct SchedulerPlan;

#[derive(Clone)]
pub struct DnsPlan {
    pub port: u16,
    pub bind_ip: IpAddr,
    pub soa_email: Option<Name>,
    pub domain_name: Option<Name>,
    pub state: StateHandle,
}

pub struct ControllerPlan {
    pub nats: TypedNats,
    pub scheduler_plan: Option<SchedulerPlan>,
    pub dns_plan: Option<DnsPlan>,
    pub state: StateHandle,
    pub http_plan: Option<HttpOptions>,
}

impl ControllerPlan {
    pub async fn from_controller_config(config: ControllerConfig) -> Result<Self> {
        let nats = config.nats.connect_with_retry("controller.inbox").await?;
        nats.initialize_jetstreams().await?;
        let state = start_state_loop(nats.clone()).await?;

        let scheduler_plan = config.scheduler.map(|_| SchedulerPlan);
        let dns_plan = if let Some(options) = config.dns {
            let soa_email = if let Some(soa_email) = options.soa_email {
                let soa_email = format_rname(&soa_email).context(
                    "soa_email provided in configuration was not a valid email address.",
                )?;
                Some(
                    Name::from_ascii(soa_email)
                        .context("soa_email contained non-ascii characters.")?,
                )
            } else {
                tracing::warn!("soa_email not set, SOA requests will fail");
                None
            };

            let domain_name = options
                .domain_name
                .and_then(|ref name| {
                    Name::from_utf8(name)
                        .map_err(|e| {
                            tracing::warn!(?e, "invalid domain name {}", name);
                            e
                        })
                        .ok()
                })
                .or_else(|| {
                    tracing::warn!("domain_name not set, SOA requests will fail");
                    None
                });

            Some(DnsPlan {
                port: options.port,
                bind_ip: options.bind_ip,
                domain_name,
                soa_email,
                state: state.clone(),
            })
        } else {
            None
        };

        let http_plan = config.http;

        Ok(ControllerPlan {
            nats,
            scheduler_plan,
            dns_plan,
            state,
            http_plan,
        })
    }
}
