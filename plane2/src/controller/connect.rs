use super::error::err_to_response;
use super::Controller;
use crate::database::connect::ConnectError;
use crate::types::{ClusterName, ConnectRequest, ConnectResponse};
use axum::{
    extract::{Path, State},
    response::Response,
    Json,
};
use reqwest::StatusCode;

fn connect_error_to_response(connect_error: &ConnectError) -> Response {
    match connect_error {
        ConnectError::FailedToAcquireLock => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to acquire lock.",
        ),
        ConnectError::LockUnheldNoSpawnConfig => err_to_response(
            connect_error,
            StatusCode::CONFLICT,
            "Lock is unheld but no spawn config was provided.",
        ),
        ConnectError::LockHeldUnhealthy => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Lock is held but unhealthy.",
        ),
        ConnectError::LockHeld { .. } => err_to_response(
            connect_error,
            StatusCode::CONFLICT,
            "Lock is held but tag does not match.",
        ),
        ConnectError::NoDroneAvailable => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "No active drone available.",
        ),
        ConnectError::FailedToRemoveLock => err_to_response(
            connect_error,
            StatusCode::CONFLICT,
            "Failed to remove lock.",
        ),
        ConnectError::Sql(_) | ConnectError::Serialization(_) => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error.",
        ),
        ConnectError::Other(_) => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error.",
        ),
    }
}

pub async fn handle_connect(
    Path(cluster): Path<ClusterName>,
    State(controller): State<Controller>,
    Json(request): Json<ConnectRequest>,
) -> Result<Json<ConnectResponse>, Response> {
    let response = controller
        .db
        .connect(&cluster, &request)
        .await
        .map_err(|e| connect_error_to_response(&e))?;
    Ok(Json(response))
}
