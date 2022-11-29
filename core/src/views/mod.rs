use crate::{
    messages::agent::{BackendStateMessage, DroneState, DroneStatusMessage},
    nats::TypedNats,
    types::{BackendId, ClusterName, DroneId},
    Never,
};
use anyhow::{anyhow, Result};
use dashmap::DashMap;
use time::OffsetDateTime;
use std::{collections::HashMap, net::Ipv4Addr, sync::Arc};
use tokio::task::JoinHandle;

pub struct DroneView {
    state: DroneState,
}

#[derive(Default)]
pub struct ClusterView {
    backends: HashMap<BackendId, (DroneId, Ipv4Addr)>,
    drones: HashMap<DroneId, DroneView>,
}

impl ClusterView {
    pub fn receive_drone_status(&mut self, status: &DroneStatusMessage) {
        if status.state == DroneState::Stopped {
            self.drones.remove(&status.drone_id);
        } else {
            self.drones.insert(status.drone_id, )
        }
        
    }
}

#[derive(Default)]
pub struct SystemView {
    clusters: HashMap<ClusterName, ClusterView>,
}

impl SystemView {
    pub fn receive_drone_status_message(&mut self, message: &DroneStatusMessage, timestamp: OffsetDateTime) -> Result<()> {
        let cluster = self.clusters.entry(message.cluster.clone()).or_default();

        cluster.receive_drone_status(message);

        Ok(())
    }

    pub fn receive_backend_state_message(&mut self, message: &BackendStateMessage, timestamp: OffsetDateTime) -> Result<()> {
        Ok(())
    }

    // pub fn new(nats: TypedNats) -> Self {
    //     let clusters: Arc<DashMap<ClusterName, ClusterView>> = Default::default();

    //     let handle = {
    //         let clusters = clusters.clone();
    //         tokio::spawn(async move {
    //             let mut drone_status_sub = nats
    //                 .subscribe_jetstream(DroneStatusMessage::subscribe_subject())
    //                 .await?;
    //             let mut backend_status_sub = nats
    //                 .subscribe_jetstream(BackendStateMessage::wildcard_subject())
    //                 .await?;

    //             loop {
    //                 tokio::select! {
    //                     status = drone_status_sub.next() => {
                            
    //                     }
    //                 }
    //             }

    //             Err::<Never, anyhow::Error>(anyhow::anyhow!("not implemented."))
    //         })
    //     };

    //     SystemView {
    //         clusters,
    //         _handle: handle,
    //     }
    // }
}
