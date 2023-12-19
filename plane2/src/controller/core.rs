use url::Url;

use crate::{
    client::PlaneClient,
    database::PlaneDatabase,
    names::{AnyNodeName, ControllerName},
    typed_socket::Handshake,
    types::{ClusterName, NodeId},
};
use std::net::IpAddr;

#[derive(Clone)]
pub struct Controller {
    pub db: PlaneDatabase,
    pub id: ControllerName,
    pub client: PlaneClient,
}

pub struct NodeHandle {
    pub id: NodeId,
    db: Option<PlaneDatabase>,
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        let db = self
            .db
            .take()
            .expect("self.db is always Some before dropped.");
        let id = self.id;
        tokio::spawn(async move {
            if let Err(err) = db.node().mark_offline(id).await {
                tracing::error!(?err, "Failed to mark node offline.");
            }
        });
    }
}

impl Controller {
    pub async fn register_node(
        &self,
        handshake: Handshake,
        cluster: Option<&ClusterName>,
        ip: IpAddr,
    ) -> Result<NodeHandle, sqlx::Error> {
        let name = AnyNodeName::try_from(handshake.name).map_err(|_| {
            sqlx::Error::Decode("Failed to decode node name from handshake.".into())
        })?;
        let kind = name.kind();
        let plane_version = handshake.version;

        let node_id = self
            .db
            .node()
            .register(cluster, &name, kind, &self.id, &plane_version, ip)
            .await?;

        Ok(NodeHandle {
            id: node_id,
            db: Some(self.db.clone()),
        })
    }

    pub async fn new(db: PlaneDatabase, id: ControllerName, controller_url: Url) -> Self {
        let client = PlaneClient::new(controller_url);

        Self { db, id, client }
    }
}
