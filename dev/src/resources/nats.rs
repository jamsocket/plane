use crate::container::{ContainerResource, ContainerSpec};
use anyhow::Result;
use dis_spawner::nats::TypedNats;
use dis_spawner::nats_connection::NatsConnection;
use std::collections::HashMap;

const NATS_TOKEN: &str = "mytoken";

pub struct NatsService {
    container: ContainerResource,
}

impl NatsService {
    fn connection_string(&self) -> String {
        format!("nats://{}@{}", NATS_TOKEN, self.container.ip)
    }

    pub async fn connection(&self) -> Result<TypedNats> {
        let nc = NatsConnection::new(self.connection_string())?;
        Ok(nc.connection().await?)
    }
}

pub async fn nats() -> Result<NatsService> {
    let spec = ContainerSpec {
        name: "nats".into(),
        image: "docker.io/nats:2.8".into(),
        environment: HashMap::new(),
        command: vec!["--jetstream".into(), "--auth".into(), NATS_TOKEN.into()],
        volumes: Vec::new(),
    };

    Ok(NatsService {
        container: ContainerResource::new(&spec).await?,
    })
}
