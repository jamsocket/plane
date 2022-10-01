mod error;

use self::error::OrDnsError;
use crate::plan::DnsPlan;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use dashmap::DashMap;
use dis_spawner::messages::dns::SetDnsRecord;
use dis_spawner::types::ClusterName;
use dis_spawner::Never;
use dis_spawner::{messages::dns::DnsRecordType, nats::TypedNats};
use error::Result;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::{
    self,
    net::{TcpListener, UdpSocket},
};
use trust_dns_server::client::rr::rdata::TXT;
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

struct ClusterDnsServer {
    name_map: Arc<DashMap<(ClusterName, String, DnsRecordType), RData>>,
    _handle: JoinHandle<anyhow::Result<()>>,
}

impl ClusterDnsServer {
    pub async fn new(nc: &TypedNats) -> Self {
        let nc = nc.clone();
        let name_map: Arc<DashMap<(ClusterName, String, DnsRecordType), RData>> = Arc::default();

        let handle = {
            let name_map = name_map.clone();

            tokio::spawn(async move {
                let mut stream = nc
                    .subscribe_jetstream(SetDnsRecord::subscribe_subject())
                    .await?;

                while let Some(v) = stream.next().await? {
                    match v.kind {
                        DnsRecordType::A => {
                            let key = (v.cluster.clone(), v.name.clone(), DnsRecordType::A);
                            let ip: Ipv4Addr = match v.value.parse() {
                                Ok(v) => v,
                                Err(error) => {
                                    tracing::warn!(
                                        ?error,
                                        ip = v.value,
                                        "Error parsing IP in SetDnsRecord request."
                                    );
                                    continue;
                                }
                            };
                            let value = RData::A(ip);
                            name_map.insert(key, value);
                        }
                        DnsRecordType::TXT => {
                            let key = (v.cluster.clone(), v.name.clone(), DnsRecordType::TXT);
                            let value = RData::TXT(TXT::new(vec![v.value]));
                            name_map.insert(key, value);
                        }
                    }
                }

                Ok(())
            })
        };

        ClusterDnsServer {
            name_map,
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

        match request.query().query_type() {
            RecordType::TXT => {
                let mut responses = Vec::new();

                if let Some(v) =
                    self.name_map
                        .get(&(cluster_name, hostname.into(), DnsRecordType::TXT))
                {
                    let name = request.query().name().clone();
                    let rdata = v.value().clone();
                    let ttl = 60;
                    let record = Record::from_rdata(name.into(), ttl, rdata);
                    responses.push(record);
                }

                Ok(responses)
            }
            RecordType::A => {
                let mut responses = Vec::new();

                if let Some(v) =
                    self.name_map
                        .get(&(cluster_name, hostname.into(), DnsRecordType::A))
                {
                    let name = request.query().name().clone();
                    let rdata = v.value().clone();
                    let ttl = 60;
                    let record = Record::from_rdata(name.into(), ttl, rdata);
                    responses.push(record);
                }

                Ok(responses)
            }
            // RecordType::SOA => {
            //     let name = request.query().name();
            //     let rdata = RData::SOA(SOA::new(
            //         Name::from_ascii(&self.spawner.nameserver)
            //             .or_dns_error(ResponseCode::ServFail, || {
            //                 format!("Failed to encode nameserver {}.", self.spawner.nameserver)
            //             })?,
            //         Name::from_ascii("paul.driftingin.space")
            //             .or_dns_error(ResponseCode::ServFail, || {
            //                 format!("Failed to encode nameserver contact.")
            //             })?,
            //         1,
            //         7200,
            //         7200,
            //         7200,
            //         7200,
            //     ));
            //     let ttl = 60;
            //     let record = Record::from_rdata(name.clone().into(), ttl, rdata);

            //     Ok(vec![record])
            // }
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
    let mut fut = ServerFuture::new(ClusterDnsServer::new(&plan.nc).await);

    let sock = UdpSocket::bind((plan.options.ip, plan.options.port))
        .await
        .context("Binding UDP port for DNS server.")?;
    fut.register_socket(sock);

    let listener = TcpListener::bind((plan.options.ip, plan.options.port))
        .await
        .context("Binding TCP port for DNS server.")?;
    fut.register_listener(listener, Duration::from_secs(10));

    fut.block_until_done()
        .await
        .context("Internal DNS error.")?;

    Err(anyhow!("DNS server terminated unexpectedly."))
}
