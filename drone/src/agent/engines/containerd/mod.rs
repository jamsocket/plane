use crate::agent::engine::{Engine, EngineBackendStatus};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bollard::image;
use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::types::v1::Status;
use containerd_client::{
    connect,
    services::v1::{tasks_client::TasksClient, version_client::VersionClient},
    tonic::transport::Channel,
    tonic::Request,
    with_namespace,
};
use futures::{stream::pending, Stream};
use plane_core::{
    messages::agent::{BackendStatsMessage, DroneLogMessage, SpawnRequest},
    types::{BackendId, ClusterName},
};
use prost_types::Any;
use std::pin::Pin;
use tokio::process::Command;

const NAMESPACE: &str = "default";

#[derive(Clone)]
pub struct ContainerdInterface {
    channel: Channel,
}

impl ContainerdInterface {
    pub async fn new() -> Result<Self> {
        let channel = connect("/run/containerd/containerd.sock")
            .await
            .context("Connecting to containerd socket.")?;

        let mut client = VersionClient::new(channel.clone());
        let resp = client
            .version(())
            .await
            .context("Getting version from containerd.")?;

        tracing::info!(version=%resp.get_ref().version, "Connected to containerd");

        Ok(Self { channel })
    }

    async fn create_container(&self, image: &str, container_name: &str) -> Result<()> {
        let client = ContainersClient::new(self.channel.clone());

        // need to create a spec like this: https://github.com/containerd/rust-extensions/blob/main/crates/client/examples/container_spec.json
        // let spec = Any {
        //     type_url: "types.containerd.io/opencontainers/runtime-spec/1/Spec".to_string(),
        //     value: spec.into_bytes(),
        // };

        // let container = Container {
        //     id: container_name,
        //     image: image.to_string(),
        //     runtime: Some(Runtime {
        //         name: "io.containerd.runc.v2".to_string(),
        //         options: None,
        //     }),
        //     spec: Some(spec),
        //     ..Default::default()
        // };

        // let req = CreateContainerRequest {
        //     container: Some(container),
        // };
        // let req = with_namespace!(req, NAMESPACE);

        // let _resp = client
        //     .create(req)
        //     .await
        //     .expect("Failed to create container");

        Ok(())
    }
}

async fn pull_image(image: &str) -> Result<()> {
    tracing::info!(%image, "Pulling image");

    let output = Command::new("ctr")
        .args(&["image", "pull", image])
        .output()
        .await?;

    if output.status.success() {
        tracing::info!("Image pulled successfully");
    } else {
        return Err(anyhow!(
            "Failed to pull image: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

#[async_trait]
impl Engine for ContainerdInterface {
    fn interrupt_stream(&self) -> Pin<Box<dyn Stream<Item = plane_core::types::BackendId> + Send>> {
        // TODO: implement
        Box::pin(pending())
    }

    async fn load(&self, spawn_request: &SpawnRequest) -> Result<()> {
        // TODO: respect pull policy
        pull_image(&spawn_request.executable.image).await?;

        Ok(())
    }

    async fn backend_status(&self, spawn_request: &SpawnRequest) -> Result<EngineBackendStatus> {
        let mut client = TasksClient::new(self.channel.clone());

        let req = containerd_client::services::v1::GetRequest {
            container_id: spawn_request.backend_id.to_resource_name(),
            exec_id: spawn_request.backend_id.to_resource_name(),
        };
        let req = with_namespace!(req, NAMESPACE);

        let task = client.get(req).await?.into_inner();
        tracing::info!(?task, "Got task");

        let Some(process) = &task.process else {
            return Ok(EngineBackendStatus::Unknown);
        };

        match process.status() {
            Status::Running => Ok(EngineBackendStatus::Running {
                addr: "123.123.123.123".parse()?,
            }),
            Status::Stopped => {
                if process.exit_status == 0 {
                    Ok(EngineBackendStatus::Exited)
                } else {
                    Ok(EngineBackendStatus::Failed {
                        code: process.exit_status as _,
                    })
                }
            }
            _ => Ok(EngineBackendStatus::Unknown),
        }
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
