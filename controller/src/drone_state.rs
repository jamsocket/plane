use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use plane_core::{
    messages::{
        drone_state::DroneStateUpdate,
        state::{
            AcmeDnsRecord, ClusterStateMessage, DroneMessage, DroneMessageType, DroneMeta,
            WorldStateMessage,
        },
    },
    nats::TypedNats,
    NeverResult,
};
use tokio::select;

fn convert_to_state_message(
    timestamp: DateTime<Utc>,
    update: &DroneStateUpdate,
) -> Vec<WorldStateMessage> {
    match update {
        DroneStateUpdate::AcmeMessage(msg) => vec![WorldStateMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: msg.value.clone(),
            }),
        }],
        DroneStateUpdate::Connect(msg) => vec![WorldStateMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::DroneMessage(DroneMessage {
                drone: msg.drone_id.clone(),
                message: DroneMessageType::Metadata(DroneMeta {
                    ip: msg.ip,
                    git_hash: msg.git_hash.clone(),
                    version: msg
                        .version
                        .clone()
                        .unwrap_or("missing_drone_id".to_string()), // Existing drones do not report a version in their connect message.
                }),
            }),
        }],
        DroneStateUpdate::DroneStatusMessage(msg) => vec![
            WorldStateMessage {
                cluster: msg.cluster.clone(),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: msg.drone_id.clone(),
                    message: DroneMessageType::State {
                        state: msg.state,
                        timestamp,
                    },
                }),
            },
            WorldStateMessage {
                cluster: msg.cluster.clone(),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: msg.drone_id.clone(),
                    message: DroneMessageType::KeepAlive { timestamp },
                }),
            },
        ],
        _ => {
            tracing::warn!(?update, "Got unhandled state machine update message.");
            vec![]
        }
    }
}

pub async fn apply_state_message(
    nats: &TypedNats,
    message: &WorldStateMessage,
) -> Result<Option<u64>> {
    let overwrite = message.overwrite();
    tracing::info!(?message, ?overwrite, "Publishing state message.");
    if overwrite {
        nats.publish_jetstream(message).await.map(|d| Some(d))
    } else {
        nats.publish_jetstream_if_subject_empty(message).await
    }
}

pub async fn monitor_drone_state(nats: TypedNats) -> NeverResult {
    let mut acme_sub = nats
        .subscribe(DroneStateUpdate::subscribe_subject_acme())
        .await?;
    tracing::info!("Subscribed to ACME DNS messages.");

    let mut connect_sub = nats
        .subscribe(DroneStateUpdate::subscribe_subject_connect())
        .await?;
    tracing::info!("Subscribed to drone connect messages.");

    let mut drone_status_sub = nats
        .subscribe(DroneStateUpdate::subscribe_subject_drone_status())
        .await?;
    tracing::info!("Subscribed to drone status messages.");

    loop {
        let message = select! {
            acme_msg = acme_sub.next() => acme_msg,
            connect_msg = connect_sub.next() => connect_msg,
            drone_status_msg = drone_status_sub.next() => drone_status_msg,
        };

        if let Some(message) = message {
            tracing::info!(message=?message.value, "Got state message from drone.");

            let state_messages = convert_to_state_message(Utc::now(), &message.value);
            for state_message in state_messages {
                apply_state_message(&nats, &state_message).await?;
            }

            message.try_respond(&true).await?;
        } else {
            return Err(anyhow!("Drone state subscription returned None."));
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use plane_core::{
        messages::{
            agent::DroneState,
            cert::SetAcmeDnsRecord,
            drone_state::{DroneConnectRequest, DroneStatusMessage},
        },
        types::{ClusterName, DroneId},
    };
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_drone_status_message() {
        let drone_id = DroneId::new_random();

        let msg = DroneStateUpdate::DroneStatusMessage(DroneStatusMessage {
            cluster: ClusterName::new("plane.test"),
            drone_id: drone_id.clone(),
            drone_version: "0.1.0".to_string(),
            ready: true,
            state: DroneState::Ready,
            running_backends: Some(3),
        });

        let timestamp = Utc::now();
        let state_message = convert_to_state_message(timestamp, &msg);

        let expected = vec![
            WorldStateMessage {
                cluster: ClusterName::new("plane.test"),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: drone_id.clone(),
                    message: DroneMessageType::State {
                        timestamp,
                        state: DroneState::Ready,
                    },
                }),
            },
            WorldStateMessage {
                cluster: ClusterName::new("plane.test"),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: drone_id,
                    message: DroneMessageType::KeepAlive { timestamp },
                }),
            },
        ];

        assert_eq!(state_message, expected);
        assert!(!state_message.first().unwrap().overwrite()); // Drone state messages should not overwrite the previous state.
        assert!(state_message.last().unwrap().overwrite());
    }

    #[test]
    fn test_acme_message() {
        let msg = DroneStateUpdate::AcmeMessage(SetAcmeDnsRecord {
            value: "test".to_string(),
            cluster: ClusterName::new("plane.test"),
        });
        let state_message = convert_to_state_message(Utc::now(), &msg);
        let expected = vec![WorldStateMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: "test".to_string(),
            }),
        }];

        assert_eq!(state_message, expected);
        assert!(state_message.first().unwrap().overwrite());
    }

    #[test]
    fn test_connect_message() {
        let msg = DroneStateUpdate::Connect(DroneConnectRequest {
            cluster: ClusterName::new("plane.test"),
            drone_id: DroneId::new("drone1234".to_string()),
            ip: IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
            version: Some("0.1.0".to_string()),
            git_hash: None,
        });

        let state_message = convert_to_state_message(Utc::now(), &msg);

        let expected = vec![WorldStateMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::DroneMessage(DroneMessage {
                drone: DroneId::new("drone1234".to_string()),
                message: plane_core::messages::state::DroneMessageType::Metadata(DroneMeta {
                    ip: IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
                    version: "0.1.0".to_string(),
                    git_hash: None,
                }),
            }),
        }];

        assert_eq!(state_message, expected);
        assert!(state_message.first().unwrap().overwrite());
    }
}
