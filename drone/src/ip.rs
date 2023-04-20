use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum IpSource {
    Literal(IpAddr),
    Api { api: String },
}

impl IpSource {
    pub async fn get_ip(&self) -> Result<IpAddr> {
        match self {
            IpSource::Literal(ip) => Ok(*ip),
            IpSource::Api { api } => {
                let result = reqwest::get(api).await?.text().await?;
                let ip: IpAddr = result.parse()?;
                Ok(ip)
            }
        }
    }
}
