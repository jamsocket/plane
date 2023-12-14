use super::{
    subscribe::{emit, NotificationPayload},
    util::MapSqlxError,
};
use crate::{
    heartbeat_consts::HEARTBEAT_INTERVAL_SECONDS,
    names::{AnyNodeName, ControllerName, NodeName},
    types::{ClusterName, NodeId, NodeKind, NodeStatus},
    PlaneVersionInfo,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{query, types::ipnetwork::IpNetwork, PgPool};
use std::net::IpAddr;

pub struct NodeDatabase<'a> {
    pool: &'a PgPool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct NodeHeartbeatNotification {
    pub node_id: NodeId,
}

impl NotificationPayload for NodeHeartbeatNotification {
    fn kind() -> &'static str {
        "node_heartbeat"
    }
}

impl<'a> NodeDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_id(
        &self,
        cluster: &ClusterName,
        name: &impl NodeName,
    ) -> sqlx::Result<Option<NodeId>> {
        let result = query!(
            r#"
            select id
            from node
            where cluster = $1 and name = $2
            "#,
            cluster.to_string(),
            name.as_str(),
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(result.map(|result| NodeId::from(result.id)))
    }

    pub async fn register(
        &self,
        cluster: Option<&ClusterName>,
        name: &AnyNodeName,
        kind: NodeKind,
        controller: &ControllerName,
        version: &PlaneVersionInfo,
        ip: IpAddr,
    ) -> sqlx::Result<NodeId> {
        let ip: IpNetwork = ip.into();
        let result = query!(
            r#"
            insert into node (cluster, name, last_status, controller, plane_version, plane_hash, last_heartbeat, kind, ip)
            values ($1, $2, $3, $4, $5, $6, now(), $7, $8)
            on conflict (cluster, name) do update set
                last_status = $3,
                controller = $4,
                plane_version = $5,
                plane_hash = $6,
                ip = $8
            returning id
            "#,
            cluster.map(|c| c.to_string()),
            name.to_string(),
            NodeStatus::Starting.to_string(),
            controller.to_string(),
            version.version,
            version.git_hash,
            kind.to_string(),
            ip,
        )
        .fetch_one(self.pool)
        .await?;

        Ok(NodeId::from(result.id))
    }

    pub async fn heartbeat(&self, node_id: NodeId, node_status: NodeStatus) -> Result<()> {
        let mut txn = self.pool.begin().await?;

        emit(&mut *txn, &NodeHeartbeatNotification { node_id }).await?;

        query!(
            r#"
            update node
            set
                last_heartbeat = now(),
                last_status = $2
            where id = $1
            "#,
            node_id.as_i32(),
            node_status.to_string(),
        )
        .execute(&mut *txn)
        .await?;

        txn.commit().await?;

        Ok(())
    }

    pub async fn mark_offline(&self, node_id: NodeId) -> Result<()> {
        query!(
            r#"
            update node
            set controller = null
            where id = $1
            "#,
            node_id.as_i32(),
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn list(&self) -> sqlx::Result<Vec<NodeRow>> {
        let record = query!(
            r#"
            select
                id,
                kind,
                cluster,
                controller,
                name,
                plane_version,
                plane_hash,
                last_heartbeat,
                last_status,
                now() as "as_of!"
            from node
            "#
        )
        .fetch_all(self.pool)
        .await?;

        let mut result = Vec::with_capacity(record.len());
        for row in record {
            result.push(NodeRow {
                id: NodeId::from(row.id),
                cluster: row.cluster.map(ClusterName::from),
                kind: NodeKind::try_from(row.kind).map_sqlx_error()?,
                controller: row
                    .controller
                    .map(|t| {
                        ControllerName::try_from(t).map_err(|_| {
                            sqlx::Error::Decode("Failed to decode controller name.".into())
                        })
                    })
                    .transpose()?,
                name: AnyNodeName::try_from(row.name)
                    .map_err(|_| sqlx::Error::Decode("Failed to decode node name.".into()))?,
                plane_version: row.plane_version,
                plane_hash: row.plane_hash,
                last_heartbeat: row.last_heartbeat,
                last_status: NodeStatus::try_from(row.last_status).map_sqlx_error()?,
                as_of: row.as_of,
            });
        }

        Ok(result)
    }
}

pub struct NodeRow {
    pub id: NodeId,
    pub cluster: Option<ClusterName>,
    pub kind: NodeKind,
    pub controller: Option<ControllerName>,
    pub name: AnyNodeName,
    pub plane_version: String,
    pub plane_hash: String,
    pub last_heartbeat: DateTime<Utc>,
    pub last_status: NodeStatus,
    as_of: DateTime<Utc>,
}

impl NodeRow {
    /// The duration since the heartbeat, as of the time of the query.
    pub fn status_age(&self) -> chrono::Duration {
        self.as_of - self.last_heartbeat
    }

    pub fn active(&self) -> bool {
        if self.controller.is_none() {
            return false;
        }

        if self.status_age().num_seconds() > HEARTBEAT_INTERVAL_SECONDS {
            return false;
        }

        self.last_status.is_active()
    }
}
