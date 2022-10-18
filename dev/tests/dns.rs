use std::{net::SocketAddr, time::Duration, str::Utf8Error};
use anyhow::{anyhow, Result};
use integration_test::integration_test;
use plane_controller::{config::DnsOptions, dns::serve_dns, plan::DnsPlan};
use plane_core::{nats::TypedNats, Never, messages::dns::{SetDnsRecord, DnsRecordType}, types::ClusterName};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::random_loopback_ip,
};
use trust_dns_resolver::{
    config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts},
    TokioAsyncResolver, error::ResolveErrorKind, lookup::TxtLookup, proto::rr::rdata::TXT
};

const DNS_PORT: u16 = 5353;

struct DnsServer {
    _guard: LivenessGuard<Result<Never, anyhow::Error>>,
    resolver: TokioAsyncResolver,
    pub nc: TypedNats,
}

impl DnsServer {
    async fn new() -> Result<Self> {
        let ip = random_loopback_ip();
        let nats = Nats::new().await?;
        let nc = nats.connection().await?;
        let options = DnsOptions {
            bind_ip: ip.into(),
            port: DNS_PORT,
        };

        let plan = DnsPlan {
            options,
            nc: nc.clone(),
        };
        let guard = expect_to_stay_alive(serve_dns(plan));

        let mut config = ResolverConfig::new();
        config.add_name_server(NameServerConfig::new(
            SocketAddr::new(ip.into(), DNS_PORT),
            Protocol::Tcp,
        ));
        let resolver = TokioAsyncResolver::tokio(config, ResolverOpts::default())?;

        Ok(DnsServer {
            _guard: guard,
            resolver,
            nc,
        })
    }

    async fn expect_no_a_record(&self, domain: &str) -> Result<()> {
        let result = self.resolver.ipv4_lookup(domain).await;

        match result {
            Err(e) => {
                match e.kind() {
                    ResolveErrorKind::NoRecordsFound { .. } => Ok(()),
                    _ => Err(anyhow!("Expected NXDOMAIN, got another error {:?}", e)),
                }
            },
            Ok(result) => {
                Err(anyhow!("Expected NXDOMAIN, got {:?}", result))
            }
        }
    }

    async fn txt_record(&self, domain: &str) -> Result<Vec<String>> {
        let result = self.resolver.txt_lookup(domain).await?;

        let c: Result<Vec<String>, Utf8Error> = result.into_iter().map(|d| {
            d.txt_data().into_iter().map(|k| std::str::from_utf8(k)).collect()
        }).collect();

        Ok(c?)
    }
}

#[integration_test]
async fn dns_bad_request() -> Result<()> {
    let dns = DnsServer::new().await?;

    dns.expect_no_a_record("foo.bar").await?;

    Ok(())
}

#[integration_test]
async fn dns_txt_record() -> Result<()> {
    let dns = DnsServer::new().await?;

    dns.nc.publish_jetstream(&SetDnsRecord {
        cluster: ClusterName::new("plane.test"),
        kind: DnsRecordType::TXT,
        name: "_acme-challenge".into(),
        value: "foobar".into(),
    }).await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.txt_record("_acme-challenge.plane.test").await?;
    assert_eq!(vec!["foobar".to_string()], result);

    Ok(())
}
