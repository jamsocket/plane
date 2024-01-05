use super::{backend::BackendActionMessage, connect::ConnectError, subscribe::emit_with_key};
use crate::{
    names::{BackendActionName, BackendName, Name},
    protocol::BackendAction,
    types::NodeId,
};
use sqlx::{PgPool, Postgres};

pub struct BackendActionDatabase {
    pool: PgPool,
}

impl BackendActionDatabase {
    pub fn new(pool: &PgPool) -> Self {
        // TODO: can't use a reference because the futures need to be Send.
        Self { pool: pool.clone() }
    }

    pub async fn ack_pending_action(
        &self,
        notification_id: &BackendActionName,
        drone_id: NodeId,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            r#"
            update "backend_action"
            set acked_at = now()
            where "id" = $1
            and "drone_id" = $2
            "#,
            notification_id.to_string(),
            drone_id.as_i32(),
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn pending_actions(
        &self,
        drone: NodeId,
    ) -> anyhow::Result<Vec<BackendActionMessage>> {
        let rows = sqlx::query!(
            r#"
            select "action"
            from "backend_action"
            where "drone_id" = $1
            and acked_at is null
            order by created_at asc
            "#,
            drone.as_i32(),
        )
        .fetch_all(&self.pool)
        .await?;

        let mut actions = Vec::new();

        for row in rows {
            let action: BackendActionMessage = serde_json::from_value(row.action)?;
            actions.push(action);
        }

        Ok(actions)
    }

    pub async fn create_pending_action(
        &self,
        backend_id: &BackendName,
        drone_id: NodeId,
        action: &BackendAction,
    ) -> Result<(), ConnectError> {
        let mut txn = self.pool.begin().await?;
        create_pending_action(&mut txn, backend_id, drone_id, action).await?;
        txn.commit().await?;
        Ok(())
    }
}

pub async fn create_pending_action(
    txn: &mut sqlx::Transaction<'_, Postgres>,
    backend_id: &BackendName,
    drone_id: NodeId,
    action: &BackendAction,
) -> Result<(), ConnectError> {
    let action_id = BackendActionName::new_random();

    let backend_action = BackendActionMessage {
        action_id: action_id.clone(),
        backend_id: backend_id.clone(),
        drone_id,
        action: action.clone(),
    };

    emit_with_key(&mut **txn, &drone_id.to_string(), &backend_action).await?;

    sqlx::query!(
        r#"
        insert into "backend_action" ("id", "action", "drone_id")
        values ($1, $2, $3)
        "#,
        action_id.to_string(),
        serde_json::to_value(&backend_action).expect("Backend action is always serializable"),
        drone_id.as_i32(),
    )
    .execute(&mut **txn)
    .await?;

    Ok(())
}
