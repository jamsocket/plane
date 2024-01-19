use super::{core::Controller, error::IntoApiError};
use crate::{
    names::BackendName,
    protocol::BackendAction,
    types::{backend_state::TerminationReason, TerminationKind},
};
use axum::{
    extract::{Path, State},
    response::Response,
    Json,
};

async fn terminate(
    controller: &Controller,
    backend_id: &BackendName,
    hard: bool,
) -> Result<(), Response> {
    let backend = controller
        .db
        .backend()
        .backend(backend_id)
        .await
        .or_internal_error("Database error")?
        .or_not_found("Backend does not exist")?;

    let kind = if hard {
        TerminationKind::Hard
    } else {
        TerminationKind::Soft
    };
    controller
        .db
        .backend_actions()
        .create_pending_action(
            backend_id,
            backend.drone_id,
            &BackendAction::Terminate {
                kind,
                reason: TerminationReason::External,
            },
        )
        .await
        .or_internal_error("Database error")?;

    Ok(())
}

pub async fn handle_soft_terminate(
    Path(backend_id): Path<BackendName>,
    State(controller): State<Controller>,
) -> Result<Json<()>, Response> {
    terminate(&controller, &backend_id, false).await?;
    Ok(Json(()))
}

pub async fn handle_hard_terminate(
    Path(backend_id): Path<BackendName>,
    State(controller): State<Controller>,
) -> Result<Json<()>, Response> {
    terminate(&controller, &backend_id, true).await?;
    Ok(Json(()))
}
