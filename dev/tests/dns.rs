use anyhow::Result;
use integration_test::integration_test;
use plane_controller::{
    dns::serve_dns, drone_state::monitor_drone_state, plan::DnsPlan, state::start_state_loop,
};
use plane_core::{messages::cert::SetAcmeDnsRecord, nats::TypedNats, types::ClusterName, Never};
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
        let state = start_state_loop(nc.clone()).await?;

        let plan = DnsPlan {
            bind_ip: ip.into(),
            port: DNS_PORT,
            soa_email: Some(Name::from_ascii("admin.plane.test.")?),
            nats: nc.clone(),
            state,
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
async fn dns_bad_request() {
    let dns = DnsServer::new().await.unwrap();

    assert!(dns.a_record("foo.bar").await.is_nxdomain());
}

#[integration_test]
async fn dns_txt_record() {
    let dns = DnsServer::new().await.unwrap();

    tokio::spawn(monitor_drone_state(dns.nc.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;

    dns.nc
        .request(&SetAcmeDnsRecord {
            cluster: ClusterName::new("plane.test"),
            value: "foobar".into(),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.txt_record("_acme-challenge.plane.test").await.unwrap();
    assert_eq!(vec!["foobar".to_string()], result);
}

#[integration_test]
async fn dns_multi_txt_record() {
    let dns = DnsServer::new().await.unwrap();

    tokio::spawn(monitor_drone_state(dns.nc.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;

    dns.nc
        .request(&SetAcmeDnsRecord {
            cluster: ClusterName::new("plane.test"),
            value: "foobar".into(),
        })
        .await
        .unwrap();

    dns.nc
        .request(&SetAcmeDnsRecord {
            cluster: ClusterName::new("plane.test"),
            value: "foobaz".into(),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = dns.txt_record("_acme-challenge.plane.test").await.unwrap();
    assert_eq!(vec!["foobar".to_string(), "foobaz".to_string()], result);
}

// #[integration_test]
// async fn dns_a_record() {
//     let dns = DnsServer::new().await.unwrap();

//     dns.nc
//         .publish(&DroneConnectRequest {
//             cluster: ClusterName::new("plane.test"),
//             ip: std::net::IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
//             drone_id: DroneId::new("louie".to_string()),
//         })
//         .await
//         .unwrap();

//     dns.nc.request(&SpawnRequest {
//         backend_id: BackendId::new("foobar".to_string()),
//         cluster: ClusterName::new("plane.test"),
//         drone_id: DroneId::new("louie".to_string()),
//         bearer_token: None,
//         max_idle_secs: Duration::from_secs(60),
//         executable: DockerExecutableConfig {
//             credentials: None,
//             image: "alpine".into(),
//             env: [].into_iter().collect(),
//             port: None,
//             resource_limits: ResourceLimits::default(),
//             pull_policy: DockerPullPolicy::IfNotPresent,
//             volume_mounts: vec![],
//         },
//         metadata: [].into_iter().collect(),
//     }).await.unwrap();

//     tokio::time::sleep(Duration::from_secs(1)).await;

//     let result = dns
//         .a_record("louie.plane.test")
//         .await
//         .expect("Couldn't fetch DNS record.");
//     assert_eq!(vec![Ipv4Addr::new(12, 12, 12, 12)], result);
// }

#[integration_test]
async fn dns_soa_record() {
    let dns = DnsServer::new().await.unwrap();

    let result = dns.soa_record("plane.test").await.unwrap();

    assert_eq!(1, result.len());

    let result = result.first().unwrap();
    assert_eq!("admin.plane.test.", &result.rname().to_ascii());
    assert_eq!("plane.test.", &result.mname().to_ascii());
}
