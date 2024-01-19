use super::Controller;
use crate::{
    controller::error::IntoApiError,
    database::{
        backend::BackendActionMessage,
        backend_key::{
            KEY_LEASE_HARD_TERMINATE_AFTER, KEY_LEASE_RENEW_AFTER, KEY_LEASE_SOFT_TERMINATE_AFTER,
        },
        subscribe::Subscription,
        PlaneDatabase,
    },
    protocol::{
        BackendAction, Heartbeat, KeyDeadlines, MessageFromDrone, MessageToDrone, RenewKeyResponse,
    },
    typed_socket::{server::new_server, TypedSocket},
    types::{ClusterName, NodeId, TerminationKind, backend_state::TerminationReason},
};
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, SocketAddr};

pub async fn handle_message_from_drone(
    msg: MessageFromDrone,
    drone_id: NodeId,
    controller: &Controller,
    sender: &mut TypedSocket<MessageToDrone>,
) -> anyhow::Result<()> {
    match msg {
        MessageFromDrone::BackendMetrics(_) => {
            //TODO: Forward backend metrics to postgres pub/sub here.
        }
        MessageFromDrone::Heartbeat(Heartbeat { local_time }) => {
            controller
                .db
                .drone()
                .heartbeat(drone_id, local_time)
                .await?;
        }
        MessageFromDrone::BackendEvent(backend_event) => {
            tracing::info!(
                event = ?backend_event,
                "Received backend event"
            );

            controller
                .db
                .backend()
                .update_status(
                    &backend_event.backend_id,
                    backend_event.status,
                    backend_event.address,
                    backend_event.exit_code,
                )
                .await?;

            sender
                .send(MessageToDrone::AckEvent {
                    event_id: backend_event.event_id,
                })
                .await?;
        }
        MessageFromDrone::AckAction { action_id } => {
            controller
                .db
                .backend_actions()
                .ack_pending_action(&action_id, drone_id)
                .await?;
        }
        MessageFromDrone::RenewKey(renew_key_request) => {
            controller
                .db
                .keys()
                .renew_key(&renew_key_request.backend)
                .await?;

            let deadlines = KeyDeadlines {
                renew_at: renew_key_request.local_time + KEY_LEASE_RENEW_AFTER,
                soft_terminate_at: renew_key_request.local_time + KEY_LEASE_SOFT_TERMINATE_AFTER,
                hard_terminate_at: renew_key_request.local_time + KEY_LEASE_HARD_TERMINATE_AFTER,
            };

            let renew_key_response = RenewKeyResponse {
                backend: renew_key_request.backend,
                deadlines: Some(deadlines),
            };

            sender
                .send(MessageToDrone::RenewKeyResponse(renew_key_response))
                .await?;
        }
    }

    Ok(())
}

pub async fn sweep_loop(db: PlaneDatabase, drone_id: NodeId) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let candidates = match db.backend().termination_candidates(drone_id).await {
            Ok(candidates) => candidates,
            Err(err) => {
                tracing::error!(?err, "Error terminating expired backends");
                continue;
            }
        };

        for candidate in candidates {
            tracing::info!(
                backend_id = %candidate.backend_id,
                expiration_time = ?candidate.expiration_time,
                allowed_idle_seconds = ?candidate.allowed_idle_seconds,
                as_of = ?candidate.as_of,
                last_keepalive = ?candidate.last_keepalive,
                "Terminating expired or idle backend"
            );

            if let Err(err) = db
                .backend_actions()
                .create_pending_action(
                    &candidate.backend_id,
                    drone_id,
                    &BackendAction::Terminate {
                        kind: TerminationKind::Soft,
                        reason: TerminationReason::Swept,
                    },
                )
                .await
            {
                tracing::error!(?err, "Error terminating backend");
            }
        }
    }
}

pub async fn drone_socket_inner(
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
    let drone_id = node_guard.id;

    let sweep_loop_handle = tokio::spawn(sweep_loop(controller.db.clone(), drone_id));

    controller.db.drone().register_drone(drone_id, true).await?;

    let mut backend_actions: Subscription<BackendActionMessage> =
        controller.db.subscribe_with_key(&drone_id.to_string());

    for pending_action in controller
        .db
        .backend_actions()
        .pending_actions(drone_id)
        .await?
    {
        let message = MessageToDrone::Action(pending_action);
        socket.send(message).await?;
    }

    loop {
        tokio::select! {
            backend_action_result = backend_actions.next() => {
                match backend_action_result {
                    Some(backend_action) => {
                        let message = MessageToDrone::Action(backend_action.payload);
                        if let Err(err) = socket.send(message).await {
                            tracing::error!(?err, "Error sending backend action to drone");
                        }
                    }
                    None => {
                        tracing::info!("Drone action channel closed");
                        break;
                    }
                }
            }
            message_from_drone_result = socket.recv() => {
                match message_from_drone_result {
                    Some(message_from_drone) => {
                        if let Err(err) = handle_message_from_drone(message_from_drone, drone_id, &controller, &mut socket).await {
                            tracing::error!(?err, "Error handling message from drone");
                        }
                    }
                    None => {
                        tracing::info!("Drone socket closed");
                        break;
                    }
                }
            }
        }
    }

    sweep_loop_handle.abort();

    Ok(())
}

pub async fn drone_socket(cluster: ClusterName, ws: WebSocket, controller: Controller, ip: IpAddr) {
    if let Err(err) = drone_socket_inner(cluster, ws, controller, ip).await {
        tracing::error!(?err, "Drone socket error");
    }
}

pub async fn handle_drone_socket(
    Path(cluster): Path<String>,
    State(controller): State<Controller>,
    connect_info: ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, Response> {
    let cluster: ClusterName = cluster
        .parse()
        .ok()
        .or_status(StatusCode::BAD_REQUEST, "Invalid cluster name")?;
    let ip = connect_info.0.ip();
    Ok(ws.on_upgrade(move |socket| drone_socket(cluster, socket, controller, ip)))
}
