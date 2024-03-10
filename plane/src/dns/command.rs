use crate::names::{AcmeDnsServerName, OrRandom};
use clap::Parser;
use url::Url;

#[derive(Parser)]
pub struct DnsOpts {
    #[clap(long)]
    name: Option<AcmeDnsServerName>,

    #[clap(long)]
    controller_url: Url,

    /// Suffix to strip from requests before looking up TXT records.
    /// E.g. if the zone is "example.com", a TXT record lookup
    /// for foo.bar.baz.example.com
    /// will return the TXT records for the cluster "foo.bar.baz".
    ///
    /// The DNS record for _acme-challenge.foo.bar.baz in this case
    /// should have a CNAME record pointing to foo.bar.baz.example.com.
    #[clap(long)]
    zone: String,

    #[clap(long, default_value = "53")]
    port: u16,
}

impl DnsOpts {
    pub fn into_config(self) -> crate::dns::DnsConfig {
        crate::dns::DnsConfig {
            name: self.name.or_random(),
            controller_url: self.controller_url,
            port: self.port,
            zone: Some(self.zone),
        }
    }
}
