use super::{cert_manager::CertManager, proxy_server::ProxyState};
use plane_common::{
    names::ProxyName,
    protocol::{MessageFromProxy, MessageToProxy, RouteInfoRequest},
    types::ClusterName,
    PlaneClient,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use valuable::Valuable;

pub struct ProxyConnection {
    handle: JoinHandle<()>,
    state: Arc<ProxyState>,
}

impl ProxyConnection {
    pub fn new(
        name: ProxyName,
        client: PlaneClient,
        cluster: ClusterName,
        mut cert_manager: CertManager,
        state: Arc<ProxyState>,
    ) -> Self {
        tracing::info!("Creating proxy connection");

        let handle = {
            let state = state.clone();

            tokio::spawn(async move {
                let mut proxy_connection = client.proxy_connection(&cluster);

                loop {
                    state.set_ready(false);
                    let mut conn = proxy_connection.connect_with_retry(&name).await;
                    state.set_ready(true);

                    let sender = conn.sender(MessageFromProxy::CertManagerRequest);
                    cert_manager.set_request_sender(move |m| {
                        if let Err(e) = sender.send(m) {
                            tracing::error!(?e, "Error sending cert manager request.");
                        }
                    });

                    let sender = conn.sender(MessageFromProxy::RouteInfoRequest);
                    state
                        .inner
                        .route_map
                        .set_sender(move |m: RouteInfoRequest| {
                            if let Err(e) = sender.send(m) {
                                tracing::error!(?e, "Error sending route info request.");
                            }
                        });
                    let sender = conn.sender(MessageFromProxy::KeepAlive);
                    state.inner.monitor.set_listener(move |backend| {
                        if let Err(err) = sender.send(backend.clone()) {
                            tracing::error!(?err, "Error sending keepalive.");
                        }
                    });

                    let mut log_interval = tokio::time::interval(Duration::from_secs(60));
                    log_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut message_counts: HashMap<&'static str, u64> = HashMap::new();

                    loop {
                        tokio::select! {
                            _ = log_interval.tick() => {
                                let (outgoing, incoming) = conn.channel_depths();
                                tracing::info!(
                                    outgoing_pending = outgoing,
                                    incoming_pending = incoming,
                                    route_info_response = message_counts.get("route_info_response").copied().unwrap_or(0),
                                    cert_manager_response = message_counts.get("cert_manager_response").copied().unwrap_or(0),
                                    backend_removed = message_counts.get("backend_removed").copied().unwrap_or(0),
                                    "Proxy channel stats (last 60s)"
                                );
                                message_counts.clear();
                            }
                            message_result = conn.recv() => {
                                let Some(message) = message_result else {
                                    break;
                                };

                                match &message {
                                    MessageToProxy::RouteInfoResponse(_) => {
                                        *message_counts.entry("route_info_response").or_insert(0) += 1;
                                    }
                                    MessageToProxy::CertManagerResponse(_) => {
                                        *message_counts.entry("cert_manager_response").or_insert(0) += 1;
                                    }
                                    MessageToProxy::BackendRemoved { .. } => {
                                        *message_counts.entry("backend_removed").or_insert(0) += 1;
                                    }
                                }

                                match message {
                                    MessageToProxy::RouteInfoResponse(response) => {
                                        state.inner.route_map.receive(response);
                                    }
                                    MessageToProxy::CertManagerResponse(response) => {
                                        tracing::info!(
                                            response = response.as_value(),
                                            "Received cert manager response"
                                        );
                                        cert_manager.receive(response);
                                    }
                                    MessageToProxy::BackendRemoved { backend } => {
                                        state.inner.route_map.remove_backend(&backend);
                                        state.inner.monitor.remove_backend(&backend);
                                    }
                                }
                            }
                        }
                    }
                }
            })
        };

        Self { handle, state }
    }

    pub fn state(&self) -> Arc<ProxyState> {
        self.state.clone()
    }
}

impl Drop for ProxyConnection {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
