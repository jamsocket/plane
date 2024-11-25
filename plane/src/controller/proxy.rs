use super::{core::Controller, error::IntoApiError};
use crate::database::{
    backend::RouteInfoResult,
    subscribe::{Notification, Subscription},
};
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use plane_client::{
    names::{BackendName, Name},
    protocol::{
        ApiErrorKind, CertManagerRequest, CertManagerResponse, MessageFromProxy, MessageToProxy,
        RouteInfoRequest, RouteInfoResponse,
    },
    typed_socket::{server::new_server, TypedSocket},
    types::{BackendState, BearerToken, ClusterName, NodeId},
};
use std::net::{IpAddr, SocketAddr};
use tokio::select;
use valuable::Valuable;

pub async fn handle_route_info_request(
    token: BearerToken,
    controller: &Controller,
    socket: &mut TypedSocket<MessageToProxy>,
) -> anyhow::Result<()> {
    match controller.db.backend().route_info_for_token(&token).await {
        // When a proxy requests a route, either:
        // 1. The route is ready, and we can send it back immediately.
        // 2. The route is not ready, and we need to wait for it to become ready.
        // 3. The route does not exist or has already terminated, and we can send back a `None`.
        Ok(RouteInfoResult::Available(route_info)) => {
            let response = RouteInfoResponse {
                token,
                route_info: Some(route_info),
            };
            if let Err(err) = socket.send(MessageToProxy::RouteInfoResponse(response)) {
                tracing::error!(?err, "Error sending route info response to proxy.");
            }
        }
        Ok(RouteInfoResult::Pending(partial_route_info)) => {
            let backend_id = partial_route_info.backend_id.clone();
            let mut sub: Subscription<BackendState> =
                controller.db.subscribe_with_key(backend_id.as_str());

            // There is a race condition if the status updated between when our last query hit and when we started the
            // subscription. It's a bit hacky, but for now we will just issue the query again.
            // We can't start the subscription first to avoid repeating the query, because we need to know the backend
            // ID to start the subscription.
            match controller.db.backend().route_info_for_token(&token).await? {
                RouteInfoResult::Available(route_info) => {
                    let response = RouteInfoResponse {
                        token,
                        route_info: Some(route_info),
                    };
                    if let Err(err) = socket.send(MessageToProxy::RouteInfoResponse(response)) {
                        tracing::error!(?err, "Error sending route info response to proxy.");
                    }
                    return Ok(());
                }
                RouteInfoResult::NotFound => {
                    let response = RouteInfoResponse {
                        token,
                        route_info: None,
                    };
                    if let Err(err) = socket.send(MessageToProxy::RouteInfoResponse(response)) {
                        tracing::error!(?err, "Error sending route info response to proxy.");
                    }
                    return Ok(());
                }
                RouteInfoResult::Pending(_) => {
                    // fall through
                }
            }

            let socket = socket.sender(MessageToProxy::RouteInfoResponse);
            tokio::spawn(async move {
                loop {
                    // Note: this timeout is arbitrary to avoid a memory leak. Under normal system operation, the critical
                    // timeout will be that of the backend failing to start. We use a large timeout to avoid it becoming
                    // the critical timeout when the system is functioning.
                    let result = match tokio::time::timeout(
                        std::time::Duration::from_secs(30 * 60 /* 30 minutes */),
                        sub.next(),
                    )
                    .await
                    {
                        Ok(Some(result)) => result,
                        Ok(None) => {
                            tracing::error!("Event subscription closed!");
                            break;
                        }
                        Err(_) => {
                            tracing::error!("Timeout waiting for backend state");
                            break;
                        }
                    };

                    let Notification { payload, .. } = result;

                    match payload {
                        BackendState::Ready { address } => {
                            let route_info = partial_route_info.set_address(address);
                            let response = RouteInfoResponse {
                                token,
                                route_info: Some(route_info),
                            };
                            if let Err(err) = socket.send(response) {
                                tracing::error!(
                                    ?err,
                                    "Error sending route info response to proxy."
                                );
                            }
                            break;
                        }
                        BackendState::Terminated { .. }
                        | BackendState::Terminating { .. }
                        | BackendState::HardTerminating { .. } => {
                            let response = RouteInfoResponse {
                                token,
                                route_info: None,
                            };
                            if let Err(err) = socket.send(response) {
                                tracing::error!(
                                    ?err,
                                    "Error sending route info response to proxy."
                                );
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            });
        }
        Ok(RouteInfoResult::NotFound) => {
            let response = RouteInfoResponse {
                token,
                route_info: None,
            };
            if let Err(err) = socket.send(MessageToProxy::RouteInfoResponse(response)) {
                tracing::error!(?err, "Error sending route info response to proxy.");
            }
        }
        Err(err) => {
            tracing::error!(?err, "Error getting route info");
        }
    };

    Ok(())
}

pub async fn handle_message_from_proxy(
    message: MessageFromProxy,
    controller: &Controller,
    socket: &mut TypedSocket<MessageToProxy>,
    cluster: &ClusterName,
    node_id: NodeId,
) -> anyhow::Result<()> {
    match message {
        MessageFromProxy::RouteInfoRequest(RouteInfoRequest { token }) => {
            handle_route_info_request(token, controller, socket).await?;
        }
        MessageFromProxy::KeepAlive(backend_id) => {
            match controller.db.backend().update_keepalive(&backend_id).await {
                Ok(true) => (),
                Ok(false) => {
                    tracing::error!(
                        ?backend_id,
                        ?node_id,
                        "Tried to update keepalive for non-existent backend"
                    );

                    socket.send(MessageToProxy::BackendRemoved {
                        backend: backend_id,
                    })?;
                }
                Err(err) => {
                    tracing::error!(
                        ?err,
                        ?backend_id,
                        ?node_id,
                        "Unhandled database error updating keepalive"
                    );
                }
            }
        }
        MessageFromProxy::CertManagerRequest(cert_manager_request) => {
            let response = match cert_manager_request {
                CertManagerRequest::CertLeaseRequest => {
                    let accepted = match controller
                        .db
                        .acme()
                        .lease_cluster_dns(cluster, node_id)
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
                        .set_cluster_dns(cluster, node_id, &txt_value)
                        .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            tracing::error!(?err, "Error setting cluster DNS");
                            // We still need to send a response.
                            false
                        }
                    };

                    CertManagerResponse::SetTxtRecordResponse { accepted }
                }
                CertManagerRequest::ReleaseCertLease => {
                    if let Err(err) = controller
                        .db
                        .acme()
                        .release_cluster_lease(cluster, node_id)
                        .await
                    {
                        tracing::error!(?err, "Error releasing cluster DNS");
                    };
                    return Ok(());
                }
            };

            tracing::info!(
                response = response.as_value(),
                "Sending cert manager response"
            );

            if let Err(err) = socket.send(MessageToProxy::CertManagerResponse(response)) {
                tracing::error!(?err, "Error sending cert manager response to proxy.");
            }
        }
    }

    Ok(())
}

pub async fn proxy_socket_inner(
    cluster: ClusterName,
    ws: WebSocket,
    controller: Controller,
    ip: IpAddr,
) -> anyhow::Result<()> {
    let mut socket = new_server(ws, controller.id.to_string()).await?;

    let handshake = socket.remote_handshake.clone();
    let node_guard = controller
        .register_node(handshake, Some(&cluster), ip)
        .await?;

    let mut event_subscription: Subscription<BackendState> = controller.db.subscribe();

    loop {
        select! {
            message_from_proxy_result = socket.recv() => {
                match message_from_proxy_result {
                    Some(message) => handle_message_from_proxy(message, &controller, &mut socket, &cluster, node_guard.id).await?,
                    None => {
                        tracing::info!("Proxy socket closed");
                        break;
                    }
                }
            },
            backend_state = event_subscription.next() => {
                match backend_state {
                    Some(Notification {
                        key: Some(backend_id),
                        payload: BackendState::Terminated { .. },
                        ..
                    }) => {
                        let backend_id = match BackendName::try_from(backend_id) {
                            Ok(backend_id) => backend_id,
                            Err(err) => {
                                tracing::error!(?err, "Error parsing backend ID from notification");
                                continue;
                            }
                        };
                        socket.send(MessageToProxy::BackendRemoved { backend: backend_id })?;
                    },
                    Some(_) => (),
                    None => {
                        // We treat this as an error, because it should never happen - the
                        // subscription will attempt to reconnect indefnitely.
                        tracing::error!("Event subscription closed!");
                    }
                }
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
    let cluster: ClusterName = cluster.parse().ok().or_status(
        StatusCode::BAD_REQUEST,
        "Invalid cluster name",
        ApiErrorKind::InvalidClusterName,
    )?;
    let ip = connect_info.ip();
    Ok(ws.on_upgrade(move |socket| proxy_socket(cluster, socket, controller, ip)))
}
