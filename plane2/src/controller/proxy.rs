use super::Controller;
use crate::{
    controller::error::IntoApiError,
    protocol::{
        CertManagerRequest, CertManagerResponse, MessageFromProxy, MessageToProxy,
        RouteInfoRequest, RouteInfoResponse,
    },
    typed_socket::{server::TypedWebsocketServer, FullDuplexChannel},
    types::{ClusterName, NodeStatus},
};
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, SocketAddr};

pub async fn proxy_socket_inner(
    cluster: ClusterName,
    ws: WebSocket,
    controller: Controller,
    ip: IpAddr,
) -> anyhow::Result<()> {
    let mut socket =
        TypedWebsocketServer::<MessageToProxy>::new(ws, controller.id.to_string()).await?;

    let handshake = socket.remote_handshake().clone();
    let node_guard = controller
        .register_node(handshake, Some(&cluster), ip)
        .await?;

    // TODO: this is a fake heartbeat until we decide how the proxy heartbeat should work.
    controller
        .db
        .node()
        .heartbeat(node_guard.id, NodeStatus::Available)
        .await?;

    loop {
        let message_from_proxy_result = socket.recv().await;
        tracing::info!(?message_from_proxy_result, "Handling message from proxy...");
        match message_from_proxy_result {
            Some(MessageFromProxy::RouteInfoRequest(RouteInfoRequest { token })) => {
                let route_info = match controller.db.backend().route_info_for_token(&token).await {
                    Ok(route_info) => route_info,
                    Err(err) => {
                        tracing::error!(?err, "Error getting route info");
                        continue;
                    }
                };

                let response = RouteInfoResponse { token, route_info };
                if let Err(err) = socket
                    .send(&MessageToProxy::RouteInfoResponse(response))
                    .await
                {
                    tracing::error!(?err, "Error sending route info response to proxy.");
                }
            }
            Some(MessageFromProxy::KeepAlive(backend_id)) => {
                if let Err(err) = controller.db.backend().update_keepalive(&backend_id).await {
                    tracing::error!(?err, "Error updating keepalive");
                }
            }
            Some(MessageFromProxy::CertManagerRequest(cert_manager_request)) => {
                let response = match cert_manager_request {
                    CertManagerRequest::CertLeaseRequest => {
                        let accepted = match controller
                            .db
                            .acme()
                            .lease_cluster_dns(&cluster, node_guard.id)
                            .await
                        {
                            Ok(result) => result,
                            Err(err) => {
                                tracing::error!(?err, "Error leasing cluster DNS");
                                false
                            }
                        };

                        CertManagerResponse::CertLeaseResponse { accepted }
                    }
                    CertManagerRequest::SetTxtRecord { txt_value } => {
                        let accepted = match controller
                            .db
                            .acme()
                            .set_cluster_dns(&cluster, node_guard.id, &txt_value)
                            .await
                        {
                            Ok(result) => result,
                            Err(err) => {
                                tracing::error!(?err, "Error setting cluster DNS");
                                continue;
                            }
                        };

                        CertManagerResponse::SetTxtRecordResponse { accepted }
                    }
                    CertManagerRequest::ReleaseCertLease => {
                        controller
                            .db
                            .acme()
                            .release_cluster_lease(&cluster, node_guard.id)
                            .await?;
                        continue;
                    }
                };

                tracing::info!(?response, "Sending cert manager response");

                if let Err(err) = socket
                    .send(&MessageToProxy::CertManagerResponse(response))
                    .await
                {
                    tracing::error!(?err, "Error sending cert manager response to proxy.");
                }
            }
            None => {
                tracing::info!("Proxy socket closed");
                break;
            }
        }
    }

    Ok(())
}

pub async fn proxy_socket(cluster: ClusterName, ws: WebSocket, controller: Controller, ip: IpAddr) {
    if let Err(err) = proxy_socket_inner(cluster, ws, controller, ip).await {
        tracing::error!(?err, "Error handling proxy socket");
    }
}

pub async fn handle_proxy_socket(
    Path(cluster): Path<String>,
    State(controller): State<Controller>,
    connect_info: ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, Response> {
    let cluster: ClusterName = cluster
        .parse()
        .ok()
        .or_status(StatusCode::BAD_REQUEST, "Invalid cluster name")?;
    let ip = connect_info.ip();
    Ok(ws.on_upgrade(move |socket| proxy_socket(cluster, socket, controller, ip)))
}
