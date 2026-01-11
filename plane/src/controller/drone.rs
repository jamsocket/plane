use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use plane_common::{
    log_types::LoggableTime,
    protocol::{
        ApiErrorKind, BackendAction, BackendActionMessage, Heartbeat, KeyDeadlines,
        MessageFromDrone, MessageToDrone, RenewKeyResponse,
    },
    typed_socket::{server::new_server, TypedSocketSender},
    types::{
        backend_state::TerminationReason, ClusterName, DronePoolName, NodeId, TerminationKind,
    },
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use valuable::Valuable;

use crate::{
    database::{
        backend_key::{
            KEY_LEASE_HARD_TERMINATE_AFTER, KEY_LEASE_RENEW_AFTER, KEY_LEASE_SOFT_TERMINATE_AFTER,
        },
        subscribe::Subscription,
        PlaneDatabase,
    },
    util::GuardHandle,
};

use super::{core::Controller, error::IntoApiError};

#[derive(Deserialize)]
pub struct DroneSocketQuery {
    pool: Option<DronePoolName>,
}

pub async fn handle_message_from_drone(
    msg: MessageFromDrone,
    drone_id: NodeId,
    controller: &Controller,
    sender: TypedSocketSender<MessageToDrone>,
) -> anyhow::Result<()> {
    match msg {
        MessageFromDrone::BackendMetrics(metrics_msg) => {
            controller.db.backend().publish_metrics(metrics_msg).await?;
        }
        MessageFromDrone::Heartbeat(Heartbeat { local_time }) => {
            controller
                .db
                .drone()
                .heartbeat(drone_id, local_time.0)
                .await?;
        }
        MessageFromDrone::BackendEvent(backend_event) => {
            tracing::debug!(event = backend_event.as_value(), "Received backend event");

            controller
                .db
                .backend()
                .update_state(&backend_event.backend_id, backend_event.state)
                .await?;

            sender.send(MessageToDrone::AckEvent {
                event_id: backend_event.event_id,
            })?;
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
                renew_at: LoggableTime(renew_key_request.local_time.0 + KEY_LEASE_RENEW_AFTER),
                soft_terminate_at: LoggableTime(
                    renew_key_request.local_time.0 + KEY_LEASE_SOFT_TERMINATE_AFTER,
                ),
                hard_terminate_at: LoggableTime(
                    renew_key_request.local_time.0 + KEY_LEASE_HARD_TERMINATE_AFTER,
                ),
            };

            let renew_key_response = RenewKeyResponse {
                backend: renew_key_request.backend,
                deadlines: Some(deadlines),
            };

            sender.send(MessageToDrone::RenewKeyResponse(renew_key_response))?;
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
                backend_id = candidate.backend_id.as_value(),
                drone_id = drone_id.as_i32(),
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

pub async fn process_pending_actions(
    db: &PlaneDatabase,
    socket: &mut TypedSocketSender<MessageToDrone>,
    drone_id: &NodeId,
) -> Result<(), anyhow::Error> {
    let mut count = 0;

    // Limit the number of pending actions to avoid overwhelming the drone.
    let pending_actions_limit = 50;

    for pending_action in db
        .backend_actions()
        .pending_actions(*drone_id, pending_actions_limit)
        .await?
    {
        let message = MessageToDrone::Action(pending_action);
        socket.send(message)?;
        count += 1;
    }

    if count > 0 {
        tracing::info!(count, "Sent pending actions to drone.");
    }
    if count == pending_actions_limit {
        tracing::warn!(count, "Query returned the maximum number of pending actions; other pending actions may be missed.");
    }

    Ok(())
}

pub async fn drone_socket_inner(
    cluster: ClusterName,
    ws: WebSocket,
    controller: Controller,
    ip: IpAddr,
    pool: DronePoolName,
) -> anyhow::Result<()> {
    let mut socket = new_server(ws, controller.id.to_string()).await?;

    let handshake = socket.remote_handshake.clone();
    let node_guard = controller
        .register_node(handshake, Some(&cluster), ip)
        .await?;
    let drone_id = node_guard.id;

    let sweep_loop_handle = tokio::spawn(sweep_loop(controller.db.clone(), drone_id));

    controller
        .db
        .drone()
        .register_drone(drone_id, true, pool)
        .await?;

    let mut backend_actions: Subscription<BackendActionMessage> =
        controller.db.subscribe_with_key(&drone_id.to_string());

    process_pending_actions(&controller.db, &mut socket.sender(), &drone_id).await?;

    let mut log_interval = tokio::time::interval(Duration::from_secs(60));
    log_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut message_counts: HashMap<&'static str, u64> = HashMap::new();

    let mut sender = socket.sender();
    let db = controller.db.clone();
    let _pending_actions_handle = GuardHandle::new(async move {
        loop {
            if let Err(err) = process_pending_actions(&db, &mut sender, &drone_id).await {
                tracing::error!(?err, "Error processing pending actions");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    loop {
        tokio::select! {
            _ = log_interval.tick() => {
                let (outgoing, incoming) = socket.channel_depths();
                tracing::info!(
                    drone_id = drone_id.as_i32(),
                    outgoing_pending = outgoing,
                    incoming_pending = incoming,
                    heartbeat = message_counts.get("heartbeat").copied().unwrap_or(0),
                    backend_event = message_counts.get("backend_event").copied().unwrap_or(0),
                    ack_action = message_counts.get("ack_action").copied().unwrap_or(0),
                    renew_key = message_counts.get("renew_key").copied().unwrap_or(0),
                    backend_metrics = message_counts.get("backend_metrics").copied().unwrap_or(0),
                    "Drone channel stats (last 60s)"
                );
                message_counts.clear();
            }
            backend_action_result = backend_actions.next() => {
                match backend_action_result {
                    Some(backend_action) => {
                        let message = MessageToDrone::Action(backend_action.payload);
                        if let Err(err) = socket.send(message) {
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
                        match &message_from_drone {
                            MessageFromDrone::Heartbeat(_) => {
                                *message_counts.entry("heartbeat").or_insert(0) += 1;
                            }
                            MessageFromDrone::BackendEvent(_) => {
                                *message_counts.entry("backend_event").or_insert(0) += 1;
                            }
                            MessageFromDrone::AckAction { .. } => {
                                *message_counts.entry("ack_action").or_insert(0) += 1;
                            }
                            MessageFromDrone::RenewKey(_) => {
                                *message_counts.entry("renew_key").or_insert(0) += 1;
                            }
                            MessageFromDrone::BackendMetrics(_) => {
                                *message_counts.entry("backend_metrics").or_insert(0) += 1;
                            }
                        }

                        let sender = socket.sender();
                        let controller = controller.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_message_from_drone(message_from_drone, drone_id, &controller, sender).await {
                                tracing::error!(?err, "Error handling message from drone");
                            }
                        });
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

pub async fn drone_socket(
    cluster: ClusterName,
    ws: WebSocket,
    controller: Controller,
    ip: IpAddr,
    pool: DronePoolName,
) {
    if let Err(err) = drone_socket_inner(cluster, ws, controller, ip, pool).await {
        tracing::error!(?err, "Drone socket error");
    }
}

pub async fn handle_drone_socket(
    Path(cluster): Path<String>,
    Query(query): Query<DroneSocketQuery>,
    State(controller): State<Controller>,
    connect_info: ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, Response> {
    let cluster: ClusterName = cluster.parse().ok().or_status(
        StatusCode::BAD_REQUEST,
        "Invalid cluster name",
        ApiErrorKind::InvalidClusterName,
    )?;
    let pool = query.pool.unwrap_or_default();
    let ip = connect_info.0.ip();
    Ok(ws.on_upgrade(move |socket| drone_socket(cluster, socket, controller, ip, pool)))
}
