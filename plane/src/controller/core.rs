use crate::database::{connect::ConnectError, PlaneDatabase};
use chrono::{DateTime, Utc};
use plane_common::{
    names::{AnyNodeName, ControllerName},
    typed_socket::Handshake,
    types::{ClusterName, ConnectRequest, ConnectResponse, NodeId},
    PlaneClient,
};
use std::net::IpAddr;
use url::Url;

#[derive(Clone)]
pub struct Controller {
    pub db: PlaneDatabase,
    pub id: ControllerName,
    pub client: PlaneClient,
    pub default_cluster: Option<ClusterName>,
    pub best_of: i64,
}

pub struct NodeHandle {
    pub id: NodeId,
    pub controller: ControllerName,
    db: Option<PlaneDatabase>,
    connection_start_time: DateTime<Utc>,
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        let db = self
            .db
            .take()
            .expect("self.db is always Some before dropped.");
        let id = self.id;
        let controller = self.controller.clone();
        let connection_start_time = self.connection_start_time;
        tokio::spawn(async move {
            if let Err(err) = db
                .node()
                .mark_offline(id, &controller, connection_start_time)
                .await
            {
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

        let (node_id, connection_start_time) = self
            .db
            .node()
            .register(cluster, &name, kind, &self.id, &plane_version, ip)
            .await?;

        Ok(NodeHandle {
            id: node_id,
            db: Some(self.db.clone()),
            controller: self.id.clone(),
            connection_start_time,
        })
    }

    pub async fn new(
        db: PlaneDatabase,
        id: ControllerName,
        controller_url: Url,
        default_cluster: Option<ClusterName>,
        best_of: i64,
    ) -> Self {
        let client = PlaneClient::new(controller_url);

        Self {
            db,
            id,
            client,
            default_cluster,
            best_of,
        }
    }

    pub async fn connect(
        &self,
        connect_request: &ConnectRequest,
    ) -> Result<ConnectResponse, ConnectError> {
        let response = self
            .db
            .connect(
                self.default_cluster.as_ref(),
                connect_request,
                &self.client,
                self.best_of,
            )
            .await?;

        Ok(response)
    }
}
