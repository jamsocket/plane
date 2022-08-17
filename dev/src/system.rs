use crate::container::{ContainerResource, ContainerSpec};
use anyhow::Result;
use std::collections::HashMap;

pub async fn nats() -> Result<ContainerResource> {
    let spec = ContainerSpec {
        name: "nats".into(),
        image: "docker.io/nats:2.8".into(),
        environment: HashMap::new(),
        command: vec!["--jetstream".into(), "--auth".into(), "mytoken".into()],
        volumes: Vec::new(),
    };

    ContainerResource::new(&spec).await
}
