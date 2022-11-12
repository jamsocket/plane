use anyhow::Result;
use integration_test::integration_test;
use plane_controller::{dns::serve_dns, plan::DnsPlan};
use plane_core::{
    messages::dns::{DnsRecordType, SetDnsRecord},
    nats::TypedNats,
    types::ClusterName,
    Never,
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::random_loopback_ip,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    str::Utf8Error,
    time::Duration,
};
use trust_dns_resolver::{
    config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts},
    error::{ResolveError, ResolveErrorKind},
    proto::rr::rdata::SOA,
    TokioAsyncResolver,
};
use trust_dns_server::client::rr::Name;

const DNS_PORT: u16 = 5353;

struct DnsServer {
    _guard: LivenessGuard<Result<Never, anyhow::Error>>,
    resolver: TokioAsyncResolver,
    pub nc: TypedNats,
}

trait DnsResultExt {
    fn is_nxdomain(&self) -> bool;
}

impl<T> DnsResultExt for Result<T, ResolveError> {
    fn is_nxdomain(&self) -> bool {
        match self {
            Err(e) => matches!(e.kind(), ResolveErrorKind::NoRecordsFound { .. }),
            Ok(_) => false,
        }
    }
}

impl DnsServer {
    async fn new() -> Result<Self> {
        let ip = random_loopback_ip();
        let nats = Nats::new().await?;
        let nc = nats.connection().await?;

        let plan = DnsPlan {
            bind_ip: ip.into(),
            port: DNS_PORT,
            soa_email: Some(Name::from_ascii("admin.plane.test.")?),
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

    async fn txt_record(&self, domain: &str) -> Result<Vec<String>> {
        let result = self.resolver.txt_lookup(domain).await?;

        let c: Result<Vec<String>, Utf8Error> = result
            .into_iter()
            .map(|d| {
                d.txt_data()
                    .iter()
                    .map(|k| std::str::from_utf8(k))
                    .collect()
            })
            .collect();

        Ok(c?)
    }

    async fn a_record(&self, domain: &str) -> std::result::Result<Vec<Ipv4Addr>, ResolveError> {
        let result = self.resolver.ipv4_lookup(domain).await?;

        Ok(result.into_iter().collect())
    }

    async fn soa_record(&self, domain: &str) -> std::result::Result<Vec<SOA>, ResolveError> {
        let result = self.resolver.soa_lookup(domain).await?;

        Ok(result.into_iter().collect())
    }
}

#[integration_test]
async fn dns_bad_request() -> Result<()> {
    let dns = DnsServer::new().await?;

    assert!(dns.a_record("foo.bar").await.is_nxdomain());

    Ok(())
}

#[integration_test]
async fn dns_txt_record() -> Result<()> {
    let dns = DnsServer::new().await?;

    dns.nc
        .publish_jetstream(&SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::TXT,
            name: "_acme-challenge".into(),
            value: "foobar".into(),
        })
        .await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.txt_record("_acme-challenge.plane.test").await?;
    assert_eq!(vec!["foobar".to_string()], result);

    Ok(())
}

#[integration_test]
async fn dns_multi_txt_record() -> Result<()> {
    let dns = DnsServer::new().await?;

    dns.nc
        .publish_jetstream(&SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::TXT,
            name: "_acme-challenge".into(),
            value: "foobar".into(),
        })
        .await?;

    dns.nc
        .publish_jetstream(&SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::TXT,
            name: "_acme-challenge".into(),
            value: "foobaz".into(),
        })
        .await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.txt_record("_acme-challenge.plane.test").await?;
    assert_eq!(vec!["foobar".to_string(), "foobaz".to_string()], result);

    Ok(())
}

#[integration_test]
async fn dns_a_record() -> Result<()> {
    let dns = DnsServer::new().await?;

    dns.nc
        .publish_jetstream(&SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::A,
            name: "louie".into(),
            value: "12.12.12.12".into(),
        })
        .await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.a_record("louie.plane.test").await?;
    assert_eq!(vec![Ipv4Addr::new(12, 12, 12, 12)], result);

    Ok(())
}

#[integration_test]
async fn dns_multi_a_record() -> Result<()> {
    let dns = DnsServer::new().await?;

    dns.nc
        .publish_jetstream(&SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::A,
            name: "louie".into(),
            value: "12.12.12.12".into(),
        })
        .await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    dns.nc
        .publish_jetstream(&SetDnsRecord {
            cluster: ClusterName::new("plane.test"),
            kind: DnsRecordType::A,
            name: "louie".into(),
            value: "14.14.14.14".into(),
        })
        .await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.a_record("louie.plane.test").await?;
    assert_eq!(vec![Ipv4Addr::new(14, 14, 14, 14)], result);

    Ok(())
}

#[integration_test]
async fn dns_soa_record() -> Result<()> {
    let dns = DnsServer::new().await?;

    let result = dns.soa_record("plane.test").await?;

    assert_eq!(1, result.len());

    let result = result.first().unwrap();
    assert_eq!("admin.plane.test.", &result.rname().to_ascii());
    assert_eq!("plane.test.", &result.mname().to_ascii());

    Ok(())
}
