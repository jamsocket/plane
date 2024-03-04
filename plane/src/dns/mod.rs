use self::{error::Result, name_to_cluster::NameToCluster};
use crate::{
    client::PlaneClient,
    dns::error::OrDnsError,
    names::AcmeDnsServerName,
    protocol::{MessageFromDns, MessageToDns},
    signals::wait_for_shutdown_signal,
    typed_socket::client::TypedSocketConnector,
    types::ClusterName,
};
use anyhow::anyhow;
use dashmap::DashMap;
use std::{net::Ipv4Addr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, UdpSocket},
    select,
    sync::broadcast::{self, channel, Sender},
    task::JoinHandle,
};
use trust_dns_server::{
    authority::MessageResponseBuilder,
    proto::{
        op::{Header, ResponseCode},
        rr::{rdata::TXT, RData, Record, RecordType},
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    ServerFuture,
};

mod error;
mod name_to_cluster;

const TCP_TIMEOUT_SECONDS: u64 = 10;

struct AcmeDnsServer {
    loop_handle: Option<JoinHandle<()>>,
    send: Sender<MessageFromDns>,
    request_map: Arc<DashMap<ClusterName, Sender<Option<String>>>>,
    name_to_cluster: NameToCluster,
}

impl AcmeDnsServer {
    fn new(
        name: AcmeDnsServerName,
        mut client: TypedSocketConnector<MessageFromDns>,
        zone: Option<String>,
    ) -> Self {
        let (send, mut recv) = broadcast::channel::<MessageFromDns>(1);
        let request_map: Arc<DashMap<ClusterName, Sender<Option<String>>>> = Arc::default();

        let loop_handle = {
            let request_map = request_map.clone();

            tokio::spawn(async move {
                loop {
                    let mut socket = client.connect_with_retry(&name).await;

                    loop {
                        select! {
                            inbound = socket.recv() => {
                                match inbound {
                                    Some(MessageToDns::TxtRecordResponse { cluster, txt_value }) => {
                                        tracing::info!(%cluster, txt_value, "Received TXT record response.");
                                        if let Some((_, sender)) = request_map.remove(&cluster) {
                                            if let Err(err) = sender.send(txt_value) {
                                                tracing::warn!(?err, "Error sending TXT record response.");
                                            }
                                        } else {
                                            tracing::warn!(?cluster, "No sender found for TXT record response.");
                                        }
                                    }
                                    None => {
                                        tracing::warn!("DNS server socket closed.");
                                        break;
                                    }
                                }
                            }
                            outbound = recv.recv() => {
                                match outbound {
                                    Ok(message) => {
                                        if let Err(err) = socket.send(message).await {
                                            tracing::warn!(?err, "Error sending message to DNS server.");
                                        }
                                    }
                                    Err(err) => {
                                        tracing::warn!(?err, "Error receiving message from broadcast channel.");
                                    }
                                }
                            }
                        }
                    }
                }
            })
        };

        Self {
            loop_handle: Some(loop_handle),
            send,
            request_map,
            name_to_cluster: NameToCluster::new(zone),
        }
    }

    async fn request(&self, cluster: ClusterName) -> anyhow::Result<Option<String>> {
        let mut receiver = match self.request_map.entry(cluster.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => entry.get().subscribe(),
            dashmap::mapref::entry::Entry::Vacant(vacant_entry) => {
                let (sender, receiver) = channel(1);
                vacant_entry.insert(sender);
                self.send
                    .send(MessageFromDns::TxtRecordRequest { cluster })?;

                receiver
            }
        };

        Ok(receiver.recv().await?)
    }

    async fn do_lookup(&self, request: &Request) -> Result<Vec<Record>> {
        let name = request.query().name().to_string();

        match request.query().query_type() {
            RecordType::TXT => {
                let Some(cluster) = self.name_to_cluster.cluster_name(&name) else {
                    tracing::warn!(
                        ?request,
                        ?name,
                        "TXT query for record that does not match configured zone."
                    );
                    return Err(error::DnsError {
                        code: ResponseCode::FormErr,
                        message: format!("Invalid TXT query: {}", name),
                    });
                };

                tracing::info!(?cluster, "TXT query for cluster.");

                let result = tokio::time::timeout(Duration::from_secs(5), self.request(cluster))
                    .await
                    .or_dns_error(ResponseCode::ServFail, || {
                        format!("Request timed out for {}", name)
                    })?
                    .or_dns_error(ResponseCode::ServFail, || {
                        format!("No TXT record found for {}", name)
                    })?;

                tracing::info!(?request, ?name, ?result, "TXT query result.");

                let result: Vec<Record> = result
                    .map(|result| {
                        Record::from_rdata(
                            request.query().name().into(),
                            300,
                            RData::TXT(TXT::new(vec![result])),
                        )
                    })
                    .into_iter()
                    .collect();

                Ok(result)
            }
            _ => {
                tracing::warn!(?request, ?name, "Unsupported query type.");
                Err(error::DnsError {
                    code: ResponseCode::NotImp,
                    message: format!("Unsupported query type: {:?}", request.query().query_type()),
                })
            }
        }
    }
}

impl Drop for AcmeDnsServer {
    fn drop(&mut self) {
        if let Some(handle) = self.loop_handle.take() {
            handle.abort()
        }
    }
}

#[async_trait::async_trait]
impl RequestHandler for AcmeDnsServer {
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

pub async fn run_dns_with_listener(
    name: AcmeDnsServerName,
    client: PlaneClient,
    listener: TcpListener,
    zone: Option<String>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut fut = ServerFuture::new(AcmeDnsServer::new(name, client.dns_connection(), zone));

    let addr = listener.local_addr()?;

    let sock = UdpSocket::bind(addr).await?;
    fut.register_socket(sock);

    fut.register_listener(
        listener,
        std::time::Duration::from_secs(TCP_TIMEOUT_SECONDS),
    );

    let (signal, future) = fut.graceful();
    let handle = tokio::spawn(future);

    tracing::info!(port=%addr.port(), "Listening for DNS queries.");
    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down DNS server.");

    signal.shutdown().await;
    handle.await??;

    Ok(())
}

pub async fn run_dns(
    name: AcmeDnsServerName,
    client: PlaneClient,
    port: u16,
    zone: Option<String>,
) -> anyhow::Result<()> {
    let ip_port_pair = (Ipv4Addr::UNSPECIFIED, port);
    let listener = TcpListener::bind(ip_port_pair).await?;
    run_dns_with_listener(name, client, listener, zone)
        .await
        .map_err(|err| anyhow!("Error running DNS server {:?}", err))
}
