use super::{
    subscribe::{emit, NotificationPayload},
    util::MapSqlxError,
};
use crate::heartbeat_consts::UNHEALTHY_SECONDS;
use crate::{
    names::{AnyNodeName, ControllerName, NodeName},
    types::{ClusterName, NodeId, NodeKind},
    PlaneVersionInfo,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::types::PgInterval, query, types::ipnetwork::IpNetwork, PgPool};
use std::{net::IpAddr, time::Duration};

pub struct NodeDatabase<'a> {
    pool: &'a PgPool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct NodeConnectionStatusChangeNotification {
    pub node_id: NodeId,
    pub connected: bool,
}

impl NotificationPayload for NodeConnectionStatusChangeNotification {
    fn kind() -> &'static str {
        "node_connection"
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
        let mut txn = self.pool.begin().await?;

        let ip: IpNetwork = ip.into();
        let result = query!(
            r#"
            insert into node (cluster, name, controller, plane_version, plane_hash, kind, ip)
            values ($1, $2, $3, $4, $5, $6, $7)
            on conflict (cluster, name) do update set
                controller = $3,
                plane_version = $4,
                plane_hash = $5,
                ip = $7
            returning id
            "#,
            cluster.map(|c| c.to_string()),
            name.to_string(),
            controller.to_string(),
            version.version,
            version.git_hash,
            kind.to_string(),
            ip,
        )
        .fetch_one(&mut *txn)
        .await?;

        emit(
            &mut *txn,
            &NodeConnectionStatusChangeNotification {
                node_id: NodeId::from(result.id),
                connected: true,
            },
        )
        .await?;

        txn.commit().await?;

        Ok(NodeId::from(result.id))
    }

    pub async fn mark_offline(&self, node_id: NodeId) -> Result<()> {
        let mut txn = self.pool.begin().await?;

        emit(
            &mut *txn,
            &NodeConnectionStatusChangeNotification {
                node_id,
                connected: false,
            },
        )
        .await?;

        query!(
            r#"
            update node
            set controller = null
            where id = $1
            "#,
            node_id.as_i32(),
        )
        .execute(&mut *txn)
        .await?;

        txn.commit().await?;

        Ok(())
    }

    pub async fn list(&self) -> sqlx::Result<Vec<NodeRow>> {
        let record = query!(
            r#"
            select
                node.id as "id!",
                kind as "kind!",
                cluster,
                (case when
                    controller.is_online and controller.last_heartbeat - now() < $1
                    then controller.id
                    else null end
                ) as controller,
                name as "name!",
                node.plane_version as "plane_version!",
                node.plane_hash as "plane_hash!"
            from node
            left join controller on controller.id = node.controller
            "#,
            PgInterval::try_from(Duration::from_secs(UNHEALTHY_SECONDS as _))
                .expect("valid interval")
        )
        .fetch_all(self.pool)
        .await?;

        let mut result = Vec::with_capacity(record.len());
        for row in record {
            result.push(NodeRow {
                id: NodeId::from(row.id),
                cluster: row
                    .cluster
                    .map(|s| {
                        s.parse().map_err(|_| {
                            sqlx::Error::Decode("Failed to decode cluster name.".into())
                        })
                    })
                    .transpose()?,
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
            });
        }

        Ok(result)
    }
}

#[derive(Debug)]
pub struct NodeRow {
    pub id: NodeId,
    pub cluster: Option<ClusterName>,
    pub kind: NodeKind,
    pub controller: Option<ControllerName>,
    pub name: AnyNodeName,
    pub plane_version: String,
    pub plane_hash: String,
}

impl NodeRow {
    pub fn active(&self) -> bool {
        self.controller.is_some()
    }
}
