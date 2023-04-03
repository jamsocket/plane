use anyhow::anyhow;
use plane_core::{
    messages::{
        drone_state::DroneStateUpdate,
        state::{
            AcmeDnsRecord, ClusterStateMessage, DroneMessage, DroneMessageType, WorldStateMessage,
        },
    },
    nats::TypedNats,
    NeverResult,
};
use tokio::select;

fn convert_to_state_message(update: &DroneStateUpdate) -> Option<WorldStateMessage> {
    match update {
        DroneStateUpdate::AcmeMessage(msg) => Some(WorldStateMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: msg.value.clone(),
            }),
        }),
        DroneStateUpdate::Connect(msg) => Some(WorldStateMessage {
            cluster: msg.cluster.clone(),
            message: ClusterStateMessage::DroneMessage(DroneMessage {
                drone: msg.drone_id.clone(),
                message: DroneMessageType::Metadata { ip: msg.ip },
            }),
        }),
        _ => {
            tracing::warn!(?update, "Got unhandled state machine update message.");
            None
        }
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

    loop {
        let message = select! {
            acme_msg = acme_sub.next() => acme_msg,
            connect_msg = connect_sub.next() => connect_msg,
        };

        if let Some(message) = message {
            tracing::info!(message=?message.value, "Got drone state message");

            let state_message = convert_to_state_message(&message.value);
            if let Some(state_message) = state_message {
                nats.publish_jetstream(&state_message).await?;
                message.respond(&true).await?;
            } else {
                tracing::warn!("Got unhandled state machine update message.");
            }
        } else {
            return Err(anyhow!("Drone state subscription returned None."));
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use plane_core::{
        messages::{cert::SetAcmeDnsRecord, drone_state::DroneConnectRequest},
        types::{ClusterName, DroneId},
    };
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_acme_message() {
        let msg = DroneStateUpdate::AcmeMessage(SetAcmeDnsRecord {
            value: "test".to_string(),
            cluster: ClusterName::new("plane.test"),
        });
        let state_message = convert_to_state_message(&msg);
        let expected = WorldStateMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                value: "test".to_string(),
            }),
        };

        assert_eq!(state_message.unwrap(), expected);
    }

    #[test]
    fn test_connect_message() {
        let msg = DroneStateUpdate::Connect(DroneConnectRequest {
            cluster: ClusterName::new("plane.test"),
            drone_id: DroneId::new("drone1234".to_string()),
            ip: IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
        });

        let state_message = convert_to_state_message(&msg);

        let expected = WorldStateMessage {
            cluster: ClusterName::new("plane.test"),
            message: ClusterStateMessage::DroneMessage(DroneMessage {
                drone: DroneId::new("drone1234".to_string()),
                message: plane_core::messages::state::DroneMessageType::Metadata {
                    ip: IpAddr::V4(Ipv4Addr::new(12, 12, 12, 12)),
                },
            }),
        };

        assert_eq!(state_message.unwrap(), expected);
    }
}
