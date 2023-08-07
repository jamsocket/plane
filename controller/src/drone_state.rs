use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use plane_core::{
    messages::{
        drone_state::DroneStateUpdate,
        state::{
            AcmeDnsRecord, BackendMessage, BackendMessageType, ClusterStateMessage, DroneMessage,
            DroneMessageType, DroneMeta, WorldStateMessage,
        },
    },
    nats::TypedNats,
    NeverResult,
};
use serde_json::Value;
use tokio::select;

fn convert_to_state_message(
    timestamp: DateTime<Utc>,
    update: &DroneStateUpdate,
) -> Vec<WorldStateMessage> {
    match update {
        DroneStateUpdate::AcmeMessage(msg) => vec![WorldStateMessage::ClusterMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: msg.value.clone(),
            }),
        }],
        DroneStateUpdate::Connect(msg) => vec![WorldStateMessage::ClusterMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::DroneMessage(DroneMessage {
                drone: msg.drone_id.clone(),
                message: DroneMessageType::Metadata(DroneMeta {
                    ip: msg.ip,
                    git_hash: msg.git_hash.clone(),
                    version: msg
                        .version
                        .clone()
                        .unwrap_or("missing_drone_version".to_string()), // Existing drones do not report a version in their connect message.
                }),
            }),
        }],
        DroneStateUpdate::DroneStatusMessage(msg) => vec![
            WorldStateMessage::ClusterMessage {
                cluster: msg.cluster.clone(),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: msg.drone_id.clone(),
                    message: DroneMessageType::State {
                        state: msg.state,
                        timestamp,
                    },
                }),
            },
            WorldStateMessage::ClusterMessage {
                cluster: msg.cluster.clone(),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: msg.drone_id.clone(),
                    message: DroneMessageType::KeepAlive { timestamp },
                }),
            },
        ],
        DroneStateUpdate::BackendStateMessage(msg) => vec![WorldStateMessage::ClusterMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::BackendMessage(BackendMessage {
                backend: msg.backend.clone(),
                message: BackendMessageType::State {
                    state: msg.state,
                    timestamp,
                },
            }),
        }],
    }
}

pub async fn apply_state_message(
    nats: &TypedNats,
    message: &WorldStateMessage,
) -> Result<Option<u64>> {
    let overwrite = message.overwrite();
    if overwrite {
        nats.publish_jetstream(message).await.map(Some)
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

    let mut backend_status_sub = nats
        .subscribe(DroneStateUpdate::subscribe_subject_backend_status())
        .await?;
    tracing::info!("Subscribed to backend status messages.");

    loop {
        let message = select! {
            acme_msg = acme_sub.next() => acme_msg,
            connect_msg = connect_sub.next() => connect_msg,
            drone_status_msg = drone_status_sub.next() => drone_status_msg,
            backend_status_msg = backend_status_sub.next() => backend_status_msg,
        };

        if let Some(message) = message {
            let state_messages = convert_to_state_message(Utc::now(), &message.value);
            for state_message in state_messages {
                apply_state_message(&nats, &state_message).await?;
            }

            // Temporary: we've consolidated handling of multiple message types:
            // - `SetAcmeDnsRecord` expects `true`
            // - `DroneConnectRequest` expects `true`
            // - `UpdateBackendStateMessage` expects `null`
            // - `DroneStatusMessage` does not have a reply inbox
            // These are all used to acknowledge the message and carry no other information.
            // Eventually, these will return an Option<u64> containing the JetStream sequence number,
            // but for now we return the appropriate type.
            let response = match message.value {
                DroneStateUpdate::BackendStateMessage(_) => None,
                _ => Some(Value::Bool(true)),
            };

            message.try_respond(&response).await?;
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
            agent::{BackendState, DroneState},
            cert::SetAcmeDnsRecord,
            drone_state::{DroneConnectRequest, DroneStatusMessage, UpdateBackendStateMessage},
            state::{BackendMessage, BackendMessageType},
        },
        types::{BackendId, ClusterName, DroneId},
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
            WorldStateMessage::ClusterMessage {
                cluster: ClusterName::new("plane.test"),
                message: ClusterStateMessage::DroneMessage(DroneMessage {
                    drone: drone_id.clone(),
                    message: DroneMessageType::State {
                        timestamp,
                        state: DroneState::Ready,
                    },
                }),
            },
            WorldStateMessage::ClusterMessage {
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
        let expected = vec![WorldStateMessage::ClusterMessage {
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

        let expected = vec![WorldStateMessage::ClusterMessage {
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

    #[test]
    fn test_backend_state_message() {
        let drone = DroneId::new_random();
        let backend = BackendId::new_random();
        let time = Utc::now();
        let msg = DroneStateUpdate::BackendStateMessage(UpdateBackendStateMessage {
            cluster: ClusterName::new("plane.test"),
            drone,
            backend: backend.clone(),
            time,
            state: BackendState::Starting,
        });

        let state_message = convert_to_state_message(time, &msg);

        let expected = vec![WorldStateMessage::ClusterMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::BackendMessage(BackendMessage {
                backend,
                message: BackendMessageType::State {
                    state: BackendState::Starting,
                    timestamp: time,
                },
            }),
        }];

        assert_eq!(state_message, expected);
        assert!(!state_message.first().unwrap().overwrite());
    }
}
