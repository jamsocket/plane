use super::{
    backend_actions::create_pending_action,
    backend_key::{KEY_LEASE_RENEW_AFTER, KEY_LEASE_SOFT_TERMINATE_AFTER},
    drone::DroneForSpawn,
};
use crate::{
    client::PlaneClient,
    database::{
        backend_key::{KeysDatabase, KEY_LEASE_EXPIRATION},
        drone::DroneDatabase,
    },
    names::{BackendName, Name},
    protocol::{AcquiredKey, BackendAction, KeyDeadlines},
    types::{
        BackendStatus, BearerToken, ClusterName, ConnectRequest, ConnectResponse, KeyConfig,
        SecretToken, SpawnConfig,
    },
    util::random_token,
};
use serde_json::{Map, Value};
use sqlx::{postgres::types::PgInterval, PgPool};
use std::time::Duration;

const TOKEN_LIFETIME_SECONDS: u64 = 3600;

type Result<T> = std::result::Result<T, ConnectError>;

#[derive(thiserror::Error, Debug)]
pub enum ConnectError {
    #[error("No active drone available.")]
    NoDroneAvailable,

    #[error("Key held and tag does not match. {request_tag:?} != {key_tag:?}")]
    KeyHeld {
        request_tag: String,
        key_tag: String,
    },

    #[error("The key is held but unhealthy.")]
    KeyHeldUnhealthy,

    #[error("The key is unheld and no spawn config was provided.")]
    KeyUnheldNoSpawnConfig,

    #[error("Failed to remove key.")]
    FailedToRemoveKey,

    #[error("Failed to acquire key.")]
    FailedToAcquireKey,

    #[error("SQL error: {0}")]
    Sql(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("No cluster provided, and no default cluster for this controller.")]
    NoClusterProvided,

    #[error("Other internal error. {0}")]
    Other(String),
}

impl ConnectError {
    /// Some errors are due to race conditions, but if we retry they should work.
    fn retryable(&self) -> bool {
        matches!(
            self,
            ConnectError::FailedToRemoveKey | ConnectError::FailedToAcquireKey
        )
    }
}

/// Attempts to create a new backend that owns the given key. If the key is already held, returns
/// Ok(None). If the key is not held, creates a new backend and returns Ok(Some(backend_id)).
async fn create_backend_with_key(
    pool: &PgPool,
    key: &KeyConfig,
    spawn_config: &SpawnConfig,
    cluster: &ClusterName,
    drone_for_spawn: &DroneForSpawn,
) -> Result<Option<BackendName>> {
    let backend_id = BackendName::new_random();
    let mut txn = pool.begin().await?;

    let result = sqlx::query!(
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
        insert into backend_key (id, key_name, namespace, tag, expires_at, fencing_token)
        select $1, $7, $8, $9, now() + $10, extract(epoch from now()) * 1000 from backend_insert
        returning fencing_token
        "#,
        backend_id.to_string(),
        cluster.to_string(),
        BackendStatus::Scheduled.to_string(),
        drone_for_spawn.id.as_i32(),
        spawn_config
            .lifetime_limit_seconds
            .map(
                |limit| PgInterval::try_from(Duration::from_secs(limit as _))
                    .expect("valid interval")
            ),
        spawn_config.max_idle_seconds,
        key.name,
        key.namespace,
        key.tag,
        PgInterval::try_from(KEY_LEASE_EXPIRATION).expect("valid constant interval"),
    )
    .fetch_optional(&mut *txn)
    .await?;

    let Some(result) = result else {
        return Ok(None);
    };

    let acquired_key = AcquiredKey {
        key: key.clone(),
        deadlines: KeyDeadlines {
            renew_at: drone_for_spawn.last_local_time + KEY_LEASE_RENEW_AFTER,
            soft_terminate_at: drone_for_spawn.last_local_time + KEY_LEASE_SOFT_TERMINATE_AFTER,
            hard_terminate_at: drone_for_spawn.last_local_time + KEY_LEASE_SOFT_TERMINATE_AFTER,
        },
        token: result.fencing_token,
    };

    let pending_action = BackendAction::Spawn {
        executable: Box::new(spawn_config.executable.clone()),
        key: acquired_key,
    };

    // Create an action to spawn the backend. If we succeed in acquiring the key,
    // this will cause the backend to spawn. If we fail to acquire the key, this
    // will be abandoned.
    create_pending_action(&mut txn, &backend_id, drone_for_spawn.id, &pending_action)
        .await
        .map_err(|e| ConnectError::Other(e.to_string()))?;

    txn.commit().await?;

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
    default_cluster: Option<&ClusterName>,
    request: &ConnectRequest,
    client: &PlaneClient,
) -> Result<ConnectResponse> {
    let key = if let Some(key) = &request.key {
        // Request includes a key, so we need to check if it is held.
        let key_result = KeysDatabase::new(pool).check_key(key).await?;

        if let Some(key_result) = key_result {
            // Key is held. Check if we can connect to existing backend.

            if key_result.is_live() {
                if key_result.tag != key.tag {
                    return Err(ConnectError::KeyHeld {
                        request_tag: key.tag.clone(),
                        key_tag: key_result.tag,
                    });
                }

                let (token, secret_token) = create_token(
                    pool,
                    &key_result.id,
                    request.user.as_deref(),
                    request.auth.clone(),
                )
                .await?;

                let connect_response = ConnectResponse::new(
                    key_result.id,
                    &key_result.cluster,
                    false,
                    key_result.status,
                    token,
                    secret_token,
                    client,
                );

                return Ok(connect_response);
            } else {
                tracing::info!("Key will be removed");

                // Key is expired. Remove it.
                let removed = KeysDatabase::new(pool).remove_key(key_result.id).await?;
                if !removed {
                    // Key was not removed, so it must have been renewed
                    // since we checked it. Return error.
                    return Err(ConnectError::FailedToRemoveKey);
                }
            }
        }

        key.clone()
    } else {
        // Request does not include a key, so we create one.
        KeyConfig::new_random()
    };

    let Some(spawn_config) = &request.spawn_config else {
        return Err(ConnectError::KeyUnheldNoSpawnConfig);
    };

    let cluster = spawn_config
        .cluster
        .as_ref()
        .or(default_cluster)
        .ok_or(ConnectError::NoClusterProvided)?;

    let drone = DroneDatabase::new(pool)
        .pick_drone_for_spawn(&cluster)
        .await?
        .ok_or(ConnectError::NoDroneAvailable)?;

    let Some(backend_id) =
        create_backend_with_key(pool, &key, spawn_config, &cluster, &drone).await?
    else {
        return Err(ConnectError::FailedToAcquireKey);
    };
    tracing::info!(backend_id = ?backend_id, "Created backend");

    let (token, secret_token) = create_token(
        pool,
        &backend_id,
        request.user.as_deref(),
        request.auth.clone(),
    )
    .await?;

    let connect_response = ConnectResponse::new(
        backend_id,
        &cluster,
        true,
        BackendStatus::Scheduled,
        token,
        secret_token,
        client,
    );

    Ok(connect_response)
}

pub async fn connect(
    pool: &PgPool,
    default_cluster: Option<&ClusterName>,
    request: &ConnectRequest,
    client: &PlaneClient,
) -> Result<ConnectResponse> {
    let mut attempt = 1;
    loop {
        match attempt_connect(pool, default_cluster, request, client).await {
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
