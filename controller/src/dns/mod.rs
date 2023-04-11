mod error;
pub mod rname_format;

use self::error::OrDnsError;
use crate::plan::DnsPlan;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use error::Result;
use plane_core::state::StateHandle;
use plane_core::types::{BackendId, ClusterName};
use plane_core::Never;
use std::net::IpAddr;
use tokio::{
    self,
    net::{TcpListener, UdpSocket},
};
use trust_dns_server::{
    authority::MessageResponseBuilder,
    client::{
        op::ResponseCode,
        rr::{
            rdata::{SOA, TXT},
            Name, RData, Record, RecordType,
        },
    },
    proto::op::Header,
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    ServerFuture,
};

const TCP_TIMEOUT_SECONDS: u64 = 10;

/// Time-to-live value set on records returned from the DNS server.
/// Not related to TTL of records used internally.
const DNS_RECORD_TTL: u32 = 60;

#[derive(PartialEq, Eq, Hash, Clone)]
struct RecordKey {
    cluster: ClusterName,
    name: String,
}

struct ClusterDnsServer {
    state: StateHandle,
    soa_email: Option<Name>,
}

impl ClusterDnsServer {
    pub async fn new(plan: &DnsPlan) -> Self {
        let state = plan.state.clone();

        ClusterDnsServer {
            state,
            soa_email: plan.soa_email.clone(),
        }
    }

    async fn do_lookup(&self, request: &Request) -> Result<Vec<Record>> {
        let name = request.query().name().to_string();
        let (backend_id, cluster_name) = name
            .split_once('.')
            .or_dns_error(ResponseCode::NXDomain, || {
                format!("Invalid name for this server {}", name)
            })?;
        let backend_id = BackendId::new(backend_id.into());
        let cluster_name = if let Some(cluster_name) = cluster_name.strip_suffix('.') {
            ClusterName::new(cluster_name)
        } else {
            ClusterName::new(cluster_name)
        };

        tracing::info!(?cluster_name, %backend_id, "Received DNS record request.");

        match request.query().query_type() {
            RecordType::TXT => {
                let mut responses = Vec::new();

                if let Some(cluster_state) = self.state.state().cluster(&cluster_name) {
                    for value in &cluster_state.txt_records {
                        let rdata = RData::TXT(TXT::new(vec![value.clone()]));
                        let record = Record::from_rdata(
                            request.query().name().into(),
                            DNS_RECORD_TTL,
                            rdata,
                        );
                        responses.push(record);
                    }
                }

                Ok(responses)
            }
            RecordType::A => {
                let mut responses = Vec::new();

                if let Some(cluster_state) = self.state.state().cluster(&cluster_name) {
                    if let Some(ip) = cluster_state.a_record_lookup(&backend_id) {
                        let name = request.query().name().clone();
                        let rdata = match ip {
                            IpAddr::V4(ip) => RData::A(ip),
                            IpAddr::V6(ip) => RData::AAAA(ip),
                        };

                        let record = Record::from_rdata(name.into(), DNS_RECORD_TTL, rdata);
                        responses.push(record);
                    }
                }

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
    let mut fut = ServerFuture::new(ClusterDnsServer::new(&plan).await);

    let ip_port_pair = (plan.bind_ip, plan.port);

    let sock = UdpSocket::bind(ip_port_pair)
        .await
        .context("Binding UDP port for DNS server.")?;
    fut.register_socket(sock);

    let listener = TcpListener::bind(ip_port_pair)
        .await
        .context("Binding TCP port for DNS server.")?;
    fut.register_listener(
        listener,
        std::time::Duration::from_secs(TCP_TIMEOUT_SECONDS),
    );

    tracing::info!(ip=%plan.bind_ip, port=%plan.port, "Listening for DNS queries.");

    fut.block_until_done()
        .await
        .context("Internal DNS error.")?;

    Err(anyhow!("DNS server terminated unexpectedly."))
}
