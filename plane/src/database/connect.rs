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
    log_types::LoggableTime,
    names::{BackendName, OrRandom},
    protocol::{AcquiredKey, BackendAction, KeyDeadlines},
    types::{
        BackendState, BackendStatus, BearerToken, ClusterName, ConnectRequest, ConnectResponse,
        DockerExecutorConfig, KeyConfig, RevokeRequest, SecretToken, SpawnConfig,
    },
    util::random_token,
};
use serde_json::{Map, Value};
use sqlx::{postgres::types::PgInterval, PgPool};
use std::time::Duration;
use valuable::Valuable;

const TOKEN_LIFETIME_SECONDS: u64 = 3600;

/// Unique violation error code in Postgres.
/// NOTE: typically we should use "on conflict do nothing", but that only
/// works with insert queries, not update queries.
/// From: https://www.postgresql.org/docs/9.2/errcodes-appendix.html
pub const PG_UNIQUE_VIOLATION_ERROR: &str = "23505";
fn violates_uniqueness(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(err) = &err {
        if let Some(code) = err.code() {
            return code == PG_UNIQUE_VIOLATION_ERROR;
        }
    }
    false
}

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
    DatabaseError(#[from] sqlx::Error),

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
/// Err(ConnectError::FailedToAcquireKey). If the key is not held, creates a new backend and
/// returns Ok(backend_id).
async fn create_backend_with_key(
    pool: &PgPool,
    key: &KeyConfig,
    spawn_config: &SpawnConfig<DockerExecutorConfig>,
    cluster: &ClusterName,
    drone_for_spawn: &DroneForSpawn,
    static_token: Option<&BearerToken>,
) -> Result<BackendName> {
    let backend_id = spawn_config.id.clone().or_random();
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
                last_keepalive,
                state,
                static_token,
                subdomain
            )
            values ($1, $2, $3, now(), $4, now() + $5, $6, now(), $11, $12, $13)
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
        serde_json::to_value(&BackendState::Scheduled).expect("valid json"),
        static_token.map(|t| t.to_string()),
        spawn_config.subdomain.as_ref().map(|s| s.to_string()),
    )
    .fetch_one(&mut *txn)
    .await;

    let result = match result {
        Ok(result) => result,
        Err(err) => {
            if violates_uniqueness(&err) {
                return Err(ConnectError::FailedToAcquireKey);
            }
            return Err(err.into());
        }
    };

    let acquired_key = AcquiredKey {
        key: key.clone(),
        deadlines: KeyDeadlines {
            renew_at: LoggableTime(drone_for_spawn.last_local_time + KEY_LEASE_RENEW_AFTER),
            soft_terminate_at: LoggableTime(
                drone_for_spawn.last_local_time + KEY_LEASE_SOFT_TERMINATE_AFTER,
            ),
            hard_terminate_at: LoggableTime(
                drone_for_spawn.last_local_time + KEY_LEASE_SOFT_TERMINATE_AFTER,
            ),
        },
        token: result.fencing_token,
    };

    let pending_action = BackendAction::Spawn {
        executable: Box::new(spawn_config.executable.clone()),
        key: acquired_key,
        static_token: static_token.cloned(),
    };

    // Create an action to spawn the backend. If we succeed in acquiring the key,
    // this will cause the backend to spawn. If we fail to acquire the key, this
    // will be abandoned.
    create_pending_action(&mut txn, &backend_id, drone_for_spawn.id, &pending_action)
        .await
        .map_err(|e| ConnectError::Other(e.to_string()))?;

    txn.commit().await?;

    Ok(backend_id)
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

pub async fn revoke(pool: &PgPool, request: &RevokeRequest) -> Result<()> {
    sqlx::query!(
        r#"
        delete from token
        where backend_id = $1 and username = $2
        "#,
        request.backend_id.to_string(),
        request.user,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn attempt_connect(
    pool: &PgPool,
    default_cluster: Option<&ClusterName>,
    request: &ConnectRequest<DockerExecutorConfig>,
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

                let (token, secret_token) = if let Some(token) = key_result.static_connection_token
                {
                    (token, None)
                } else {
                    let (token, secret_token) = create_token(
                        pool,
                        &key_result.id,
                        request.user.as_deref(),
                        request.auth.clone(),
                    )
                    .await?;

                    (token, Some(secret_token))
                };

                let connect_response = ConnectResponse::new(
                    key_result.id,
                    &key_result.cluster,
                    false,
                    key_result.status,
                    token,
                    secret_token,
                    key_result.subdomain,
                    client,
                    None,
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
        .pick_drone_for_spawn(cluster, &spawn_config.pool)
        .await?
        .ok_or(ConnectError::NoDroneAvailable)?;

    // If the spawn config specifies a static token, create one and use it.
    // Note that if this is non-None, the call to create_token below will be skipped.
    let bearer_token = spawn_config
        .use_static_token
        .then(BearerToken::new_random_static);

    let backend_id = create_backend_with_key(
        pool,
        &key,
        spawn_config,
        cluster,
        &drone,
        bearer_token.as_ref(),
    )
    .await?;
    tracing::info!(backend_id = backend_id.as_value(), "Created backend");

    let (token, secret_token) = if let Some(token) = bearer_token {
        (token, None)
    } else {
        let (token, secret_token) = create_token(
            pool,
            &backend_id,
            request.user.as_deref(),
            request.auth.clone(),
        )
        .await?;

        (token, Some(secret_token))
    };

    let connect_response = ConnectResponse::new(
        backend_id,
        cluster,
        true,
        BackendStatus::Scheduled,
        token,
        secret_token,
        spawn_config.subdomain.clone(),
        client,
        Some(drone.drone),
    );

    Ok(connect_response)
}

pub async fn connect(
    pool: &PgPool,
    default_cluster: Option<&ClusterName>,
    request: &ConnectRequest<DockerExecutorConfig>,
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

pub async fn clean_up_tokens(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    let result = sqlx::query!(
        r#"
        delete from token
        where expiration_time < now()
        "#,
    )
    .execute(pool)
    .await?;

    let row_count = result.rows_affected();
    tracing::info!(row_count, "Cleaned up expired tokens");

    Ok(())
}
