use self::error::Result;
use crate::{
    client::PlaneClient,
    dns::error::OrDnsError,
    names::AcmeDnsServerName,
    protocol::{MessageFromDns, MessageToDns},
    signals::wait_for_shutdown_signal,
    typed_socket::{client::TypedSocketConnector, FullDuplexChannel},
    types::ClusterId,
};
use dashmap::DashMap;
use std::{net::Ipv4Addr, sync::Arc};
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

const TCP_TIMEOUT_SECONDS: u64 = 10;

struct AcmeDnsServer {
    loop_handle: Option<JoinHandle<()>>,
    send: Sender<MessageFromDns>,
    request_map: Arc<DashMap<ClusterId, Sender<Option<String>>>>,
}

impl AcmeDnsServer {
    fn new(name: AcmeDnsServerName, mut client: TypedSocketConnector<MessageFromDns>) -> Self {
        let (send, mut recv) = broadcast::channel::<MessageFromDns>(1);
        let request_map: Arc<DashMap<ClusterId, Sender<Option<String>>>> = Arc::default();

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
                                        if let Err(err) = socket.send(&message).await {
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
        }
    }

    async fn request(&self, cluster: ClusterId) -> anyhow::Result<Option<String>> {
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
                tracing::info!(?request, ?name, "TXT query.");
                let name = name.strip_suffix('.').unwrap_or(&name);
                let Some(name) = name.strip_prefix("_acme-challenge.") else {
                    tracing::warn!(?request, ?name, "TXT query on non _acme-challenge domain.");
                    return Err(error::DnsError {
                        code: ResponseCode::FormErr,
                        message: format!("Invalid TXT query: {}", name),
                    });
                };

                let cluster = ClusterId::from(name.to_string());
                let result = self
                    .request(cluster)
                    .await
                    .or_dns_error(ResponseCode::ServFail, || {
                        format!("No TXT record found for {}", name)
                    })?;

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
        self.loop_handle.take().map(|handle| handle.abort());
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
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut fut = ServerFuture::new(AcmeDnsServer::new(name, client.dns_connection()));

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
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let ip_port_pair = (Ipv4Addr::UNSPECIFIED, port);
    let listener = TcpListener::bind(ip_port_pair).await?;
    run_dns_with_listener(name, client, listener).await
}
