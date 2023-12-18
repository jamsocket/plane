use crate::{
    client::PlaneClient,
    database::{
        backend_key::{BackendKeyHealth, KeysDatabase},
        drone::DroneDatabase,
    },
    names::{BackendName, Name},
    protocol::BackendAction,
    types::{
        BackendStatus, BearerToken, ClusterName, ConnectRequest, ConnectResponse, KeyConfig,
        NodeId, SecretToken, SpawnConfig,
    },
    util::random_token,
};
use serde_json::{Map, Value};
use sqlx::{postgres::types::PgInterval, PgPool};
use std::time::Duration;

use super::backend_actions::create_pending_action;

const TOKEN_LIFETIME_SECONDS: u64 = 3600;

type Result<T> = std::result::Result<T, ConnectError>;

#[derive(thiserror::Error, Debug)]
pub enum ConnectError {
    #[error("No active drone available.")]
    NoDroneAvailable,

    #[error("Lock held and tag does not match. {request_tag:?} != {lock_tag:?}")]
    LockHeld {
        request_tag: String,
        lock_tag: String,
    },

    #[error("The lock is held but unhealthy.")]
    LockHeldUnhealthy,

    #[error("The lock is unheld and no spawn config was provided.")]
    LockUnheldNoSpawnConfig,

    #[error("Failed to remove lock.")]
    FailedToRemoveLock,

    #[error("Failed to acquire lock.")]
    FailedToAcquireLock,

    #[error("SQL error: {0}")]
    Sql(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Other internal error. {0}")]
    Other(String),
}

impl ConnectError {
    /// Some errors are due to race conditions, but if we retry they should work.
    fn retryable(&self) -> bool {
        matches!(
            self,
            ConnectError::FailedToRemoveLock | ConnectError::FailedToAcquireLock
        )
    }
}

/// Attempts to create a new backend that owns the given lock. If the lock is already held, returns
/// Ok(None). If the lock is not held, creates a new backend and returns Ok(Some(backend_id)).
async fn create_backend_with_lock(
    pool: &PgPool,
    lock: &KeyConfig,
    spawn_config: &SpawnConfig,
    cluster: &ClusterName,
    drone: NodeId,
) -> Result<Option<BackendName>> {
    let backend_id = BackendName::new_random();
    let mut txn = pool.begin().await?;

    let pending_action = BackendAction::Spawn {
        executable: spawn_config.executable.clone(),
        key: lock.clone(),
    };

    create_pending_action(&mut txn, &backend_id, drone, &pending_action)
        .await
        .map_err(|e| ConnectError::Other(e.to_string()))?;

    let backend_result = sqlx::query!(
        r#"
        with backend_insert as (
            insert into backend (
                id,
                cluster,
                last_status,
                last_status_time,
                drone_id,
                expiration_time,
                allowed_idle_seconds,
                last_keepalive
            )
            values ($1, $2, $3, now(), $4, now() + $5, $6, now())
            returning id
        )
        insert into backend_key (backend_id, cluster, key_name, namespace, tag, last_renewed)
        select $1, $2, $7, $8, $9, now() from backend_insert
        "#,
        backend_id.to_string(),
        cluster.to_string(),
        BackendStatus::Scheduled.to_string(),
        drone.as_i32(),
        spawn_config
            .lifetime_limit_seconds
            .map(
                |limit| PgInterval::try_from(Duration::from_secs(limit as _))
                    .expect("valid interval")
            ),
        spawn_config.max_idle_seconds,
        lock.name,
        lock.namespace,
        lock.tag,
    )
    .execute(&mut *txn)
    .await?;

    txn.commit().await?;

    if backend_result.rows_affected() == 0 {
        return Ok(None);
    }

    Ok(Some(backend_id))
}

async fn create_token(
    pool: &PgPool,
    backend: &BackendName,
    user: Option<&str>,
    auth: Map<String, Value>,
) -> Result<(BearerToken, SecretToken)> {
    let token = random_token();
    let secret_token = random_token();

    sqlx::query!(
        r#"
        insert into token (token, backend_id, username, auth, secret_token, expiration_time)
        values ($1, $2, $3, $4, $5, now() + $6)
        "#,
        token,
        backend.to_string(),
        user,
        serde_json::to_value(auth).expect("json map is always serializable"),
        secret_token,
        PgInterval::try_from(Duration::from_secs(TOKEN_LIFETIME_SECONDS)).expect("valid interval"),
    )
    .execute(pool)
    .await?;

    Ok((BearerToken::from(token), SecretToken::from(secret_token)))
}

async fn attempt_connect(
    pool: &PgPool,
    cluster: &ClusterName,
    request: &ConnectRequest,
    client: &PlaneClient,
) -> Result<ConnectResponse> {
    let lock = if let Some(lock) = &request.key {
        // Request includes a lock, so we need to check if it is held.
        let lock_result = KeysDatabase::new(pool).check_key(cluster, lock).await?;

        if let Some(lock_result) = lock_result {
            // Lock is held. Check if we can connect to existing backend.

            let lock_action = lock_result.key_health();

            match lock_action {
                BackendKeyHealth::Active(backend_id) => {
                    if lock_result.tag != lock.tag {
                        return Err(ConnectError::LockHeld {
                            request_tag: lock.tag.clone(),
                            lock_tag: lock_result.tag,
                        });
                    }

                    let (token, secret_token) = create_token(
                        pool,
                        &backend_id,
                        request.user.as_deref(),
                        request.auth.clone(),
                    )
                    .await?;

                    let connect_response = ConnectResponse::new(
                        backend_id,
                        cluster,
                        false,
                        token,
                        secret_token,
                        client,
                    );

                    return Ok(connect_response);
                }
                BackendKeyHealth::Unhealthy => {
                    tracing::info!("Lock is unhealthy");
                    // Lock is unhealthy but cannot be removed; return error.
                    return Err(ConnectError::LockHeldUnhealthy);
                }
                BackendKeyHealth::Expired => {
                    tracing::info!("Lock will be removed");

                    // Lock is expired. Remove it.
                    let removed = KeysDatabase::new(pool).remove_key(lock_result.id).await?;
                    if !removed {
                        // Lock was not removed, so it must have been renewed
                        // since we checked it. Return error.
                        return Err(ConnectError::FailedToRemoveLock);
                    }
                }
            }
        }

        lock.clone()
    } else {
        // Request does not include a lock, so we create one.
        KeyConfig::new_random()
    };

    let Some(spawn_config) = &request.spawn_config else {
        return Err(ConnectError::LockUnheldNoSpawnConfig);
    };

    let drone = DroneDatabase::new(pool)
        .pick_drone_for_spawn(cluster)
        .await?
        .ok_or(ConnectError::NoDroneAvailable)?;

    let Some(backend_id) =
        create_backend_with_lock(pool, &lock, spawn_config, cluster, drone).await?
    else {
        return Err(ConnectError::FailedToAcquireLock);
    };
    tracing::info!(backend_id = ?backend_id, "Created backend");

    let (token, secret_token) = create_token(
        pool,
        &backend_id,
        request.user.as_deref(),
        request.auth.clone(),
    )
    .await?;

    let connect_response =
        ConnectResponse::new(backend_id, cluster, true, token, secret_token, client);

    Ok(connect_response)
}

pub async fn connect(
    pool: &PgPool,
    cluster: &ClusterName,
    request: &ConnectRequest,
    client: &PlaneClient,
) -> Result<ConnectResponse> {
    let mut attempt = 1;
    loop {
        match attempt_connect(pool, cluster, request, client).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                if !error.retryable() || attempt >= 3 {
                    return Err(error);
                }
                tracing::info!(error = ?error, attempt, "Retrying connect");
                attempt += 1;
            }
        }
    }
}
