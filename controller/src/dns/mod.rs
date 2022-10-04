mod error;

use self::error::OrDnsError;
use crate::plan::DnsPlan;
use crate::ttl_store::ttl_map::TtlMap;
use crate::ttl_store::ttl_multistore::TtlMultistore;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use dis_spawner::messages::dns::SetDnsRecord;
use dis_spawner::types::ClusterName;
use dis_spawner::Never;
use dis_spawner::{messages::dns::DnsRecordType, nats::TypedNats};
use error::Result;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio::{
    self,
    net::{TcpListener, UdpSocket},
};
use trust_dns_server::client::rr::rdata::TXT;
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

#[derive(PartialEq, Eq, Hash, Clone)]
struct RecordKey {
    cluster: ClusterName,
    name: String,
}

struct ClusterDnsServer {
    a_record_map: Arc<Mutex<TtlMap<RecordKey, RData>>>,
    txt_record_map: Arc<Mutex<TtlMultistore<RecordKey, RData>>>,
    _handle: JoinHandle<anyhow::Result<()>>,
}

impl ClusterDnsServer {
    pub async fn new(nc: &TypedNats) -> Self {
        let nc = nc.clone();
        let a_record_map: Arc<Mutex<TtlMap<RecordKey, RData>>> =
            Arc::new(Mutex::new(TtlMap::new(Duration::seconds(60))));
        let txt_record_map: Arc<Mutex<TtlMultistore<RecordKey, RData>>> =
            Arc::new(Mutex::new(TtlMultistore::new(Duration::seconds(60))));

        let handle = {
            let a_record_map = a_record_map.clone();
            let txt_record_map = txt_record_map.clone();

            tokio::spawn(async move {
                let mut stream = nc
                    .subscribe_jetstream(SetDnsRecord::subscribe_subject())
                    .await?;

                while let Some(v) = stream.next().await? {
                    match v.kind {
                        DnsRecordType::A => {
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
                            a_record_map
                                .lock()
                                .expect("a_record_map was poisoned")
                                .insert(
                                    RecordKey {
                                        cluster: v.cluster.clone(),
                                        name: v.name.clone(),
                                    },
                                    value,
                                    Utc::now(),
                                )
                        }
                        DnsRecordType::TXT => {
                            let value = RData::TXT(TXT::new(vec![v.value]));
                            txt_record_map
                                .lock()
                                .expect("txt_record_map was poisoned")
                                .insert(
                                    RecordKey {
                                        cluster: v.cluster.clone(),
                                        name: v.name.clone(),
                                    },
                                    value,
                                    Utc::now(),
                                );
                        }
                    }
                }

                Ok(())
            })
        };

        ClusterDnsServer {
            a_record_map,
            txt_record_map,
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

                if let Some(v) = self
                    .txt_record_map
                    .lock()
                    .expect("txt_record_map was poisoned")
                    .iter(
                        &RecordKey {
                            cluster: cluster_name,
                            name: hostname.into(),
                        },
                        Utc::now(),
                    )
                {
                    let name: Name = request.query().name().clone().into();
                    let ttl = 60;

                    for rdata in v {
                        let record = Record::from_rdata(name.clone(), ttl, rdata.clone());
                        responses.push(record);
                    }
                }

                Ok(responses)
            }
            RecordType::A => {
                let mut responses = Vec::new();

                if let Some(v) = self
                    .a_record_map
                    .lock()
                    .expect("a_record_map was poisoned")
                    .get(
                        &RecordKey {
                            cluster: cluster_name,
                            name: hostname.into(),
                        },
                        Utc::now(),
                    )
                {
                    let name = request.query().name().clone();
                    let rdata = v.clone();
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

    let ip_port_pair = (plan.options.bind_ip, plan.options.port);

    let sock = UdpSocket::bind(ip_port_pair)
        .await
        .context("Binding UDP port for DNS server.")?;
    fut.register_socket(sock);

    let listener = TcpListener::bind(ip_port_pair)
        .await
        .context("Binding TCP port for DNS server.")?;
    fut.register_listener(listener, std::time::Duration::from_secs(10));

    tracing::info!(ip=%plan.options.bind_ip, port=%plan.options.port, "Listening for DNS queries.");

    fut.block_until_done()
        .await
        .context("Internal DNS error.")?;

    Err(anyhow!("DNS server terminated unexpectedly."))
}
