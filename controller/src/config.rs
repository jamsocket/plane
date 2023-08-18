use plane_core::{nats_connection::NatsConnectionSpec, types::ClusterName};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
};

#[derive(Serialize, Deserialize)]
pub struct SchedulerOptions {}

#[derive(Serialize, Deserialize)]
pub struct DnsOptions {
    #[serde(default = "default_dns_port")]
    pub port: u16,

    #[serde(default = "default_bind_ip")]
    pub bind_ip: IpAddr,

    /// Email address to use as RNAME in the SOA record.
    /// See [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035#section-3.3.13).
    /// Note that in the DNS protocol, the email is returned in zone-file format,
    /// however, the email provided here should be a normal "username@domain.tld"-format
    /// email.
    pub soa_email: Option<String>,

    /// A domain name whose A record is the IP of this DNS server
    pub domain_name: Option<String>,
}

fn default_dns_port() -> u16 {
    53
}

fn default_http_port() -> u16 {
    9090
}

fn default_bind_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HttpOptions {
    #[serde(default = "default_http_port")]
    pub port: u16,

    #[serde(default = "default_bind_ip")]
    pub bind_ip: IpAddr,

    pub services: HashMap<String, String>,

    pub cluster: ClusterName,
}

#[derive(Serialize, Deserialize)]
pub struct ControllerConfig {
    /// How to connect to NATS.
    pub nats: NatsConnectionSpec,

    pub scheduler: Option<SchedulerOptions>,

    pub dns: Option<DnsOptions>,

    pub http: Option<HttpOptions>,
}
