use anyhow::Result;
use chrono::Utc;
use integration_test::integration_test;
use plane_controller::{dns::serve_dns, plan::DnsPlan};
use plane_core::{
    messages::state::{
        AcmeDnsRecord, BackendMessage, BackendMessageType, ClusterStateMessage, DroneMessage,
        DroneMessageType, DroneMeta, WorldStateMessage,
    },
    state::{StateHandle, WorldState},
    types::{BackendId, ClusterName, DroneId},
    Never,
};
use plane_dev::{
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::random_loopback_ip,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    str::Utf8Error,
};
use trust_dns_resolver::{
    config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts},
    error::{ResolveError, ResolveErrorKind},
    proto::rr::rdata::SOA,
    TokioAsyncResolver,
};
use trust_dns_server::client::rr::Name;

const DNS_PORT: u16 = 5354;

struct DnsServer {
    _guard: LivenessGuard<Result<Never, anyhow::Error>>,
    resolver: TokioAsyncResolver,
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
    fn new(state: WorldState) -> Result<Self> {
        let ip = random_loopback_ip();
        let state = StateHandle::new(state);

        let plan = DnsPlan {
            bind_ip: ip.into(),
            port: DNS_PORT,
            domain_name: Some(Name::from_ascii("ns1.plane.test.")?),
            soa_email: Some(Name::from_ascii("admin.plane.test.")?),
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

    async fn ns_record(&self, domain: &str) -> std::result::Result<Vec<Name>, ResolveError> {
        let result = self.resolver.ns_lookup(domain).await?;

        Ok(result.into_iter().collect())
    }
}

#[integration_test]
async fn dns_bad_request() {
    let state = WorldState::default();
    let dns = DnsServer::new(state).unwrap();

    assert!(dns.a_record("foo.bar").await.is_nxdomain());
}

#[integration_test]
async fn dns_txt_record() {
    let mut state = WorldState::default();
    state.apply(
        WorldStateMessage::ClusterMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: "foobar".into(),
            }),
        },
        1,
        Utc::now(),
    );
    let dns = DnsServer::new(state).unwrap();
    let result = dns.txt_record("_acme-challenge.plane.test").await.unwrap();
    assert_eq!(vec!["foobar".to_string()], result);
}

#[integration_test]
async fn dns_multi_txt_record() {
    let mut state = WorldState::default();
    state.apply(
        WorldStateMessage::ClusterMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: "foobar".into(),
            }),
        },
        1,
        Utc::now(),
    );
    state.apply(
        WorldStateMessage::ClusterMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: "foobaz".into(),
            }),
        },
        2,
        Utc::now(),
    );
    let dns = DnsServer::new(state).unwrap();

    let result = dns.txt_record("_acme-challenge.plane.test").await.unwrap();
    assert_eq!(vec!["foobar".to_string(), "foobaz".to_string()], result);
}

#[integration_test]
async fn dns_a_record() {
    let drone_id = DroneId::new_random();
    let mut state = WorldState::default();
    state.apply(
        WorldStateMessage::ClusterMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::DroneMessage(DroneMessage {
                drone: drone_id.clone(),
                message: DroneMessageType::Metadata(DroneMeta {
                    ip: Ipv4Addr::new(12, 12, 12, 12).into(),
                    version: "0.1.0".into(),
                    git_hash: None,
                }),
            }),
        },
        1,
        Utc::now(),
    );
    state.apply(
        WorldStateMessage::ClusterMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::BackendMessage(BackendMessage {
                backend: BackendId::new("louie".to_string()),
                message: BackendMessageType::Assignment {
                    lock_assignment: None,
                    drone: drone_id.clone(),
                    bearer_token: None,
                },
            }),
        },
        2,
        Utc::now(),
    );

    let dns = DnsServer::new(state).unwrap();

    let result = dns
        .a_record("louie.plane.test")
        .await
        .expect("Couldn't fetch DNS record.");
    assert_eq!(vec![Ipv4Addr::new(12, 12, 12, 12)], result);
}

#[integration_test]
async fn dns_soa_record() {
    let dns = DnsServer::new(WorldState::default()).unwrap();

    let result = dns.soa_record("something.plane.test").await.unwrap();

    assert_eq!(1, result.len());

    let result = result.first().unwrap();
    assert_eq!("admin.plane.test.", &result.rname().to_ascii());
    assert_eq!("ns1.plane.test.", &result.mname().to_ascii());
}

#[integration_test]
async fn dns_ns_record() {
    let dns = DnsServer::new(WorldState::default()).unwrap();

    let result = dns.ns_record("plane.test").await.unwrap();

    assert_eq!(1, result.len());
    assert_eq!("ns1.plane.test.", &result.first().unwrap().to_ascii());
}
