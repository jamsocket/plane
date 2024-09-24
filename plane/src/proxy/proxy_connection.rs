use super::{cert_manager::CertManager, proxy_server::ProxyState};
use crate::{
    client::PlaneClient,
    names::ProxyName,
    protocol::{MessageFromProxy, MessageToProxy, RouteInfoRequest},
    types::ClusterName,
};
use std::sync::Arc;
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
    ) -> Self {
        let state = Arc::new(ProxyState::new());

        let handle = {
            let state = state.clone();

            tokio::spawn(async move {
                let mut proxy_connection = client.proxy_connection(&cluster);

                loop {
                    let mut conn = proxy_connection.connect_with_retry(&name).await;

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

                    while let Some(message) = conn.recv().await {
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
