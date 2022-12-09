mod error;
pub mod rname_format;

use self::error::OrDnsError;
use crate::plan::DnsPlan;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use dashmap::mapref::one::RefMut;
use dashmap::DashMap;
use error::Result;
use plane_core::messages::cert::SetAcmeDnsRecord;
use plane_core::types::BackendId;
use plane_core::types::ClusterName;
use plane_core::views::replica::SystemViewReplica;
use plane_core::Never;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::task::JoinHandle;
use tokio::{
    self,
    net::{TcpListener, UdpSocket},
};
use trust_dns_server::client::rr::rdata::{SOA, TXT};
use trust_dns_server::client::rr::Name;
use trust_dns_server::{
    authority::MessageResponseBuilder,
    client::{
        op::ResponseCode,
        rr::{RData, Record, RecordType},
    },
    proto::op::Header,
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    ServerFuture,
};

/// Connection timeout for incoming TCP connections.
const TCP_TIMEOUT_SECONDS: u64 = 10;

/// Time-to-live value set on records returned from the DNS server.
/// Not related to TTL of records used internally.
const DNS_RECORD_TTL: u32 = 60;

#[derive(Clone)]
struct MultiRecordMap {
    map: Arc<DashMap<ClusterName, VecDeque<(Instant, String)>>>,
    ttl: Duration,
}

impl MultiRecordMap {
    fn new(ttl: Duration) -> Self {
        Self {
            map: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub fn insert(&self, now: Instant, cluster_name: ClusterName, value: String) {
        let mut map = self.map.entry(cluster_name).or_default();
        map.push_back((now, value));
    }

    pub fn get(
        &self,
        now: Instant,
        cluster_name: &ClusterName,
    ) -> Option<RefMut<ClusterName, VecDeque<(Instant, String)>>> {
        // Remove expired records from the front of the queue.
        let mut map = self.map.get_mut(cluster_name)?;
        while let Some((t, _)) = map.front() {
            if *t + self.ttl < now {
                map.pop_front();
            } else {
                break;
            }
        }

        Some(map)
    }
}

struct ClusterDnsServer {
    txt_record_map: MultiRecordMap,
    view: SystemViewReplica,
    soa_email: Option<Name>,
    _handle: JoinHandle<anyhow::Result<()>>,
}

impl ClusterDnsServer {
    pub async fn new(plan: DnsPlan) -> Self {
        let nc = plan.nc.clone();
        let txt_record_map: MultiRecordMap = MultiRecordMap::new(SetAcmeDnsRecord::ttl());

        let handle = {
            let txt_record_map = txt_record_map.clone();

            tokio::spawn(async move {
                tracing::info!("In SetDnsRecord subscription loop.");

                loop {
                    let mut stream = nc.subscribe(SetAcmeDnsRecord::subscribe_subject()).await?;

                    while let Some(v) = stream.next().await {
                        let v = v.value;
                        tracing::info!(?v, "Got SetAcmeDnsRecord request.");

                        // let value = RData::TXT(TXT::new(vec![v.value]));
                        txt_record_map.insert(Instant::now(), v.cluster, v.value);
                    }

                    tracing::warn!("SetDnsRecord connection lost; reconnecting.");
                }
            })
        };

        ClusterDnsServer {
            txt_record_map,
            soa_email: plan.soa_email.clone(),
            view: plan.view,
            _handle: handle,
        }
    }

    async fn do_lookup(&self, request: &Request) -> Result<Vec<Record>> {
        let name = request.query().name().to_string();
        let (hostname, cluster_name) = name
            .split_once('.')
            .or_dns_error(ResponseCode::NXDomain, || {
                format!("Invalid name for this server {}", name)
            })?;
        let cluster_name = if let Some(cluster_name) = cluster_name.strip_suffix('.') {
            ClusterName::new(cluster_name)
        } else {
            ClusterName::new(cluster_name)
        };

        tracing::info!(?cluster_name, %hostname, "Received DNS record request.");

        match request.query().query_type() {
            RecordType::TXT => {
                let mut responses = Vec::new();

                if let Some(v) = self.txt_record_map.get(Instant::now(), &cluster_name) {
                    let name: Name = request.query().name().clone().into();
                    for (_, value) in v.iter() {
                        let rdata = RData::TXT(TXT::new(vec![value.clone()]));
                        let record =
                            Record::from_rdata(name.clone(), DNS_RECORD_TTL, rdata.clone());
                        responses.push(record);
                    }
                }

                Ok(responses)
            }
            RecordType::A => {
                let responses = if let Some(cluster) = self.view.view().cluster(&cluster_name) {
                    if let Some(ip) = cluster.route(&BackendId::new(hostname.to_string())) {
                        let name = request.query().name().clone();

                        match ip {
                            IpAddr::V4(ip) => {
                                let rdata = RData::A(ip);
                                let record = Record::from_rdata(name.into(), DNS_RECORD_TTL, rdata);
                                vec![record]
                            }
                            IpAddr::V6(ip) => {
                                let rdata = RData::AAAA(ip);
                                let record = Record::from_rdata(name.into(), DNS_RECORD_TTL, rdata);
                                vec![record]
                            }
                        }
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };

                Ok(responses)
            }
            RecordType::SOA => {
                let name = request.query().name();
                let soa_record = self
                    .soa_email
                    .as_ref()
                    .or_dns_error(ResponseCode::ServFail, || {
                        "SOA record email not set in config.".to_string()
                    })?;

                let rdata = RData::SOA(SOA::new(
                    name.into(),
                    soa_record.clone(),
                    1,
                    7200,
                    7200,
                    7200,
                    7200,
                ));
                let ttl = 60;
                let record = Record::from_rdata(name.clone().into(), ttl, rdata);

                Ok(vec![record])
            }
            RecordType::CAA | RecordType::AAAA => Ok(vec![]), // Not supported but don't report.
            request => {
                tracing::info!(?request, "Unhandled lookup type {}.", request);
                Ok(vec![])
            }
        }
    }
}

#[async_trait]
impl RequestHandler for ClusterDnsServer {
    async fn handle_request<R>(&self, request: &Request, mut response_handle: R) -> ResponseInfo
    where
        R: ResponseHandler,
    {
        let builder = MessageResponseBuilder::from_message_request(request);
        let mut header = Header::response_from_request(request.header());

        let result = match self.do_lookup(request).await {
            Ok(answers) => {
                let response = builder.build(header, answers.iter(), vec![], vec![], vec![]);
                response_handle.send_response(response).await
            }
            Err(e) => {
                tracing::warn!(?e, ?request, "Error encountered processing DNS request.");

                header.set_response_code(e.code);
                let response = builder.build_no_records(header);
                response_handle.send_response(response).await
            }
        };

        if let Ok(result) = result {
            result
        } else {
            tracing::warn!(?request, "send_response failed in DNS handling.");
            ResponseInfo::from(header)
        }
    }
}

pub async fn serve_dns(plan: DnsPlan) -> anyhow::Result<Never> {
    let addr = SocketAddr::new(plan.bind_ip, plan.port);
    let mut fut = ServerFuture::new(ClusterDnsServer::new(plan).await);

    let sock = UdpSocket::bind(addr)
        .await
        .context("Binding UDP port for DNS server.")?;
    fut.register_socket(sock);

    let listener = TcpListener::bind(addr)
        .await
        .context("Binding TCP port for DNS server.")?;
    fut.register_listener(
        listener,
        std::time::Duration::from_secs(TCP_TIMEOUT_SECONDS),
    );

    tracing::info!(%addr, "Listening for DNS queries.");

    fut.block_until_done()
        .await
        .context("Internal DNS error.")?;

    Err(anyhow!("DNS server terminated unexpectedly."))
}
