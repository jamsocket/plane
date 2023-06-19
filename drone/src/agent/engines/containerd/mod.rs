use std::pin::Pin;
use anyhow::Result;
use async_trait::async_trait;
use containerd_client::{tonic::{transport::Channel}, services::v1::version_client::VersionClient, connect};
use futures::Stream;
use crate::agent::engine::{Engine, EngineBackendStatus};
use plane_core::{
    messages::agent::{
        BackendStatsMessage, DroneLogMessage,
        SpawnRequest,
    },
    types::{BackendId, ClusterName},
};


#[derive(Clone)]
pub struct ContainerdInterface {
    channel: Channel,
}

impl ContainerdInterface {
    pub async fn new() -> Result<Self> {
        let channel = connect("/run/containerd/containerd.sock").await?;

        let mut client = VersionClient::new(channel.clone());
        let resp = client.version(()).await?;

        tracing::info!(version=%resp.get_ref().version, "Connected to containerd");
    
        Ok(Self { channel })
    }
}

#[async_trait]
impl Engine for ContainerdInterface {
    fn interrupt_stream(&self) -> Pin<Box<dyn Stream<Item = plane_core::types::BackendId> + Send>> {
        todo!()
    }

    async fn load(&self, spawn_request: &SpawnRequest) -> Result<()> {
        todo!()
    }

    async fn backend_status(&self, spawn_request: &SpawnRequest) -> Result<EngineBackendStatus> {
        todo!()
    }

    fn log_stream(
        &self,
        backend: &BackendId,
    ) -> Pin<Box<dyn Stream<Item = DroneLogMessage> + Send>> {
        todo!()
    }

    fn stats_stream(
        &self,
        backend: &BackendId,
        cluster: &ClusterName,
    ) -> Pin<Box<dyn Stream<Item = BackendStatsMessage> + Send>> {
        todo!()
    }

    async fn stop(&self, backend: &BackendId) -> Result<()> {
        todo!()
    }
}
