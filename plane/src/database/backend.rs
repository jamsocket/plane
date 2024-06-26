use super::{
    subscribe::{emit_ephemeral_with_key, emit_with_key},
    PlaneDatabase,
};
use crate::{
    log_types::BackendAddr,
    names::{BackendActionName, BackendName},
    protocol::{BackendAction, RouteInfo},
    types::{
        backend_state::BackendStatusStreamEntry, BackendState, BackendStatus, BearerToken,
        ClusterName, NodeId, SecretToken, Subdomain,
    },
};
use chrono::{DateTime, Utc};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use std::{fmt::Debug, net::SocketAddr, str::FromStr};
use valuable::Valuable;

pub struct BackendDatabase<'a> {
    db: &'a PlaneDatabase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendActionMessage {
    pub action_id: BackendActionName,
    pub backend_id: BackendName,
    pub drone_id: NodeId,
    pub action: BackendAction,
}

impl super::subscribe::NotificationPayload for BackendActionMessage {
    fn kind() -> &'static str {
        "backend_action"
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackendMetricsMessage {
    pub backend_id: BackendName,
    /// Memory used by backend excluding inactive file cache, same as use shown by docker stats
    /// ref: https://github.com/docker/cli/blob/master/cli/command/container/stats_helpers.go#L227C45-L227C45
    pub mem_used: u64,
    /// Memory used by backend in bytes
    /// (calculated using kernel memory used by cgroup + page cache memory used by cgroup)
    pub mem_total: u64,
    /// Active memory (non reclaimable)
    pub mem_active: u64,
    /// Inactive memory (reclaimable)
    pub mem_inactive: u64,
    /// Unevictable memory (mlock etc)
    pub mem_unevictable: u64,
    /// The backend's memory limit
    pub mem_limit: u64,
    /// Nanoseconds of CPU used by backend since last message
    pub cpu_used: u64,
    /// Total CPU nanoseconds for system since last message
    pub sys_cpu: u64,
}

impl super::subscribe::NotificationPayload for BackendMetricsMessage {
    fn kind() -> &'static str {
        "backend_metrics"
    }
}

impl super::subscribe::NotificationPayload for BackendState {
    fn kind() -> &'static str {
        "backend_state"
    }
}

impl<'a> BackendDatabase<'a> {
    pub fn new(db: &'a PlaneDatabase) -> Self {
        Self { db }
    }

    pub async fn status_stream(
        &self,
        backend: &BackendName,
    ) -> sqlx::Result<impl Stream<Item = BackendStatusStreamEntry>> {
        let mut sub = self
            .db
            .subscribe_with_key::<BackendState>(&backend.to_string());

        let result = sqlx::query!(
            r#"
            select
                id,
                created_at,
                state
            from backend_state
            where backend_id = $1
            order by id asc
            "#,
            backend.to_string(),
        )
        .fetch_all(&self.db.pool)
        .await?;

        let stream = async_stream::stream! {
            let mut last_status = None;
            for row in result {
                let state: Result<BackendState, _> = serde_json::from_value(row.state);
                match state {
                    Ok(state) => {
                        yield BackendStatusStreamEntry::from_state(state.clone(), row.created_at);
                        last_status = Some(state.status());
                    }
                    Err(e) => {
                        tracing::warn!(?e, "Invalid backend status");
                    }
                }
            }

            while let Some(item) = sub.next().await {
                let state = item.payload;
                // In order to missing events that occur when we read the DB and when we subscribe to updates,
                // we subscribe to updates before we read from the DB. But this means we might get duplicate
                // events, so we keep track of the last status we saw and ignore events that have a status
                // less than or equal to it.
                if let Some(last_status) = last_status {
                    if state.status() <= last_status {
                        continue;
                    }
                }

                let time = item.timestamp;
                let item = BackendStatusStreamEntry::from_state(state.clone(), time);

                last_status = Some(state.status());

                yield item;
            }
        };

        Ok(stream)
    }

    pub async fn backend(&self, backend_id: &BackendName) -> sqlx::Result<Option<BackendRow>> {
        let result = sqlx::query!(
            r#"
            select
                id,
                cluster,
                last_status,
                last_status_time,
                state,
                drone_id,
                expiration_time,
                allowed_idle_seconds,
                last_keepalive,
                now() as "as_of!"
            from backend
            where id = $1
            "#,
            backend_id.to_string(),
        )
        .fetch_optional(&self.db.pool)
        .await?;

        let Some(result) = result else {
            return Ok(None);
        };

        Ok(Some(BackendRow {
            id: BackendName::try_from(result.id)
                .map_err(|_| sqlx::Error::Decode("Failed to decode backend name.".into()))?,
            cluster: result.cluster,
            last_status_time: result.last_status_time,
            last_keepalive: result.last_keepalive,
            state: serde_json::from_value(result.state)
                .map_err(|_| sqlx::Error::Decode("Failed to decode backend state.".into()))?,
            drone_id: NodeId::from(result.drone_id),
            expiration_time: result.expiration_time,
            allowed_idle_seconds: result.allowed_idle_seconds,
            as_of: result.as_of,
        }))
    }

    pub async fn update_state(
        &self,
        backend: &BackendName,
        new_state: BackendState,
    ) -> sqlx::Result<bool> {
        let mut txn = self.db.pool.begin().await?;

        let new_status = new_state.status();
        let new_status_number = new_status.as_int();

        let result = sqlx::query!(
            r#"
            update backend
            set
                last_status = $2,
                last_status_time = now(),
                last_status_number = $3,
                cluster_address = $4,
                state = $5
            where id = $1
            and (last_status_number < $3 or last_status_number is null)
            "#,
            backend.to_string(),
            new_status.to_string(),
            new_status_number,
            new_state.address().map(|d| d.0.to_string()),
            serde_json::to_value(&new_state)
                .expect("BackendState should always be JSON-serializable."),
        )
        .execute(&mut *txn)
        .await?;

        if result.rows_affected() == 0 {
            let result = sqlx::query!(
                r#"
                select last_status
                from backend
                where id = $1
                "#,
                backend.to_string(),
            )
            .fetch_optional(&mut *txn)
            .await?;

            let last_status = result.map(|r| r.last_status);

            tracing::warn!(last_status, new_status=%new_status, backend=backend.as_value(), "Not updating backend status");
            return Ok(false);
        }

        // If the backend is terminated, we can delete its associated key.
        if matches!(new_state, BackendState::Terminated { .. }) {
            sqlx::query!(
                r#"
                delete from backend_key
                where id = $1
                "#,
                backend.to_string(),
            )
            .execute(&mut *txn)
            .await?;
        }

        emit_state_change(&mut txn, backend, &new_state).await?;

        txn.commit().await?;

        Ok(true)
    }

    pub async fn list_backends(&self) -> sqlx::Result<Vec<BackendRow>> {
        let query_result = sqlx::query!(
            r#"
            select
                id,
                cluster,
                last_status,
                last_status_time,
                state,
                drone_id,
                expiration_time,
                allowed_idle_seconds,
                last_keepalive,
                now() as "as_of!"
            from backend
            "#
        )
        .fetch_all(&self.db.pool)
        .await?;

        let mut result = Vec::new();

        for row in query_result {
            result.push(BackendRow {
                id: BackendName::try_from(row.id)
                    .map_err(|_| sqlx::Error::Decode("Failed to decode backend name.".into()))?,
                cluster: row.cluster,
                last_status_time: row.last_status_time,
                state: serde_json::from_value(row.state)
                    .map_err(|_| sqlx::Error::Decode("Failed to decode backend state.".into()))?,
                last_keepalive: row.last_keepalive,
                drone_id: NodeId::from(row.drone_id),
                expiration_time: row.expiration_time,
                allowed_idle_seconds: row.allowed_idle_seconds,
                as_of: row.as_of,
            });
        }

        Ok(result)
    }

    pub async fn route_info_for_static_token(
        &self,
        token: &BearerToken,
    ) -> sqlx::Result<Option<RouteInfo>> {
        let result = sqlx::query!(
            r#"
            select
                id,
                cluster,
                last_status,
                cluster_address,
                subdomain
            from backend
            where backend.static_token = $1
            limit 1
            "#,
            token.to_string(),
        )
        .fetch_optional(&self.db.pool)
        .await?;

        let Some(result) = result else {
            return Ok(None);
        };

        let Some(address) = result.cluster_address else {
            return Ok(None);
        };

        let Ok(address) = address.parse::<SocketAddr>() else {
            tracing::warn!("Invalid cluster address: {}", address);
            return Ok(None);
        };

        Ok(Some(RouteInfo {
            backend_id: BackendName::try_from(result.id)
                .map_err(|_| sqlx::Error::Decode("Failed to decode backend name.".into()))?,
            address: BackendAddr(address),
            secret_token: SecretToken::from("".to_string()),
            user: None,
            user_data: None,
            cluster: ClusterName::from_str(&result.cluster)
                .map_err(|_| sqlx::Error::Decode("Failed to decode cluster name.".into()))?,
            subdomain: result
                .subdomain
                .map(Subdomain::try_from)
                .transpose()
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
        }))
    }

    pub async fn route_info_for_token(
        &self,
        token: &BearerToken,
    ) -> sqlx::Result<Option<RouteInfo>> {
        if token.is_static() {
            return self.route_info_for_static_token(token).await;
        }

        let result = sqlx::query!(
            r#"
            select
                backend_id,
                username,
                auth,
                cluster,
                last_status,
                cluster_address,
                secret_token,
                subdomain
            from token
            inner join backend
            on backend.id = token.backend_id
            and backend.last_status = $2
            where token = $1
            limit 1
            "#,
            token.to_string(),
            BackendStatus::Ready.to_string(),
        )
        .fetch_optional(&self.db.pool)
        .await?;

        let Some(result) = result else {
            return Ok(None);
        };

        let Some(address) = result.cluster_address else {
            return Ok(None);
        };

        let Ok(address) = address.parse::<SocketAddr>() else {
            tracing::warn!("Invalid cluster address: {}", address);
            return Ok(None);
        };

        Ok(Some(RouteInfo {
            backend_id: BackendName::try_from(result.backend_id)
                .map_err(|_| sqlx::Error::Decode("Failed to decode backend name.".into()))?,
            address: BackendAddr(address),
            secret_token: SecretToken::from(result.secret_token),
            user: result.username,
            user_data: Some(result.auth),
            cluster: ClusterName::from_str(&result.cluster)
                .map_err(|_| sqlx::Error::Decode("Failed to decode cluster name.".into()))?,
            subdomain: result
                .subdomain
                .map(Subdomain::try_from)
                .transpose()
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
        }))
    }

    pub async fn update_keepalive(&self, backend_id: &BackendName) -> sqlx::Result<()> {
        let result = sqlx::query!(
            r#"
            update backend
            set
                last_keepalive = now()
            where id = $1
            "#,
            backend_id.to_string(),
        )
        .execute(&self.db.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }

        Ok(())
    }

    pub async fn publish_metrics(&self, metrics: BackendMetricsMessage) -> sqlx::Result<()> {
        let mut txn = self.db.pool.begin().await?;
        emit_ephemeral_with_key(&mut txn, &metrics.backend_id.to_string(), &metrics).await?;
        txn.commit().await?;
        Ok(())
    }

    pub async fn termination_candidates(
        &self,
        drone_id: NodeId,
    ) -> sqlx::Result<Vec<TerminationCandidate>> {
        let result = sqlx::query!(
            r#"
            select
                id as backend_id,
                expiration_time,
                allowed_idle_seconds,
                last_keepalive,
                now() as "as_of!"
            from backend
            where
                drone_id = $1
                and last_status not in ($2, $3)
                and (
                    now() - last_keepalive > make_interval(secs => allowed_idle_seconds)
                    or now() > expiration_time
                )
            "#,
            drone_id.as_i32(),
            BackendStatus::Scheduled.to_string(),
            BackendStatus::Terminated.to_string(),
        )
        .fetch_all(&self.db.pool)
        .await?;

        let mut candidates = Vec::new();
        for row in result {
            candidates.push(TerminationCandidate {
                backend_id: BackendName::try_from(row.backend_id)
                    .map_err(|_| sqlx::Error::Decode("Failed to decode backend name.".into()))?,
                expiration_time: row.expiration_time,
                last_keepalive: row.last_keepalive,
                allowed_idle_seconds: row.allowed_idle_seconds,
                as_of: row.as_of,
            });
        }

        Ok(candidates)
    }

    pub async fn cleanup(&self, min_age_days: i32) -> sqlx::Result<()> {
        tracing::info!("Cleaning up terminated backends.");
        let mut txn = self.db.pool.begin().await?;

        sqlx::query(
            r#"
            create temporary table deleted_backend on commit drop as (
                select id from backend
                where
                    last_status = $1
                    and now() - last_status_time > make_interval(days => $2)
            );
            "#,
        )
        .bind(BackendStatus::Terminated.to_string())
        .bind(min_age_days)
        .execute(&mut *txn)
        .await?;

        let token_result = sqlx::query(
            r#"
            delete from token
            where token.backend_id in (select id from deleted_backend);
            "#,
        )
        .execute(&mut *txn)
        .await?;

        let token_deleted = token_result.rows_affected();

        let backend_action_result = sqlx::query(
            r#"
            delete from backend_action
            where backend_action.backend_id in (select id from deleted_backend);
            "#,
        )
        .execute(&mut *txn)
        .await?;

        let backend_action_deleted = backend_action_result.rows_affected();

        let backend_state_result = sqlx::query(
            r#"
            delete from backend_state
            where backend_state.backend_id in (select id from deleted_backend);
            "#,
        )
        .execute(&mut *txn)
        .await?;

        let backend_state_deleted = backend_state_result.rows_affected();

        let backend_result = sqlx::query(
            r#"
            delete from backend
            where id in (select id from deleted_backend);
            "#,
        )
        .execute(&mut *txn)
        .await?;

        let backend_deleted = backend_result.rows_affected();

        txn.commit().await?;

        tracing::info!(
            token_deleted,
            backend_action_deleted,
            backend_state_deleted,
            backend_deleted,
            "Finished cleanup."
        );

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TerminationCandidate {
    pub backend_id: BackendName,
    pub expiration_time: Option<DateTime<Utc>>,
    pub last_keepalive: DateTime<Utc>,
    pub allowed_idle_seconds: Option<i32>,
    pub as_of: DateTime<Utc>,
}

pub struct BackendRow {
    pub id: BackendName,
    pub cluster: String,
    pub last_status_time: DateTime<Utc>,
    pub state: BackendState,
    pub last_keepalive: DateTime<Utc>,
    pub drone_id: NodeId,
    pub expiration_time: Option<DateTime<Utc>>,
    pub allowed_idle_seconds: Option<i32>,
    pub as_of: DateTime<Utc>,
}

impl BackendRow {
    /// The duration since the heartbeat, as of the time of the query.
    pub fn status_age(&self) -> chrono::Duration {
        self.as_of - self.last_status_time
    }
}

/// Update the backend_state table, without updating the backend table.
pub async fn emit_state_change(
    txn: &mut PgConnection,
    backend: &BackendName,
    new_state: &BackendState,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        insert into backend_state (backend_id, state)
        values ($1, $2)
        "#,
        backend.to_string(),
        serde_json::to_value(&new_state).expect("BackendState should always be JSON-serializable."),
    )
    .execute(&mut *txn)
    .await?;

    emit_with_key(txn, &backend.to_string(), new_state).await?;

    Ok(())
}
