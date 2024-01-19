use super::error::err_to_response;
use super::Controller;
use crate::database::connect::ConnectError;
use crate::types::{ConnectRequest, ConnectResponse};
use axum::{extract::State, response::Response, Json};
use reqwest::StatusCode;

fn connect_error_to_response(connect_error: &ConnectError) -> Response {
    match connect_error {
        ConnectError::FailedToAcquireKey => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to acquire lock.",
        ),
        ConnectError::KeyUnheldNoSpawnConfig => err_to_response(
            connect_error,
            StatusCode::CONFLICT,
            "Lock is unheld but no spawn config was provided.",
        ),
        ConnectError::KeyHeldUnhealthy => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Lock is held but unhealthy.",
        ),
        ConnectError::KeyHeld { .. } => err_to_response(
            connect_error,
            StatusCode::CONFLICT,
            "Lock is held but tag does not match.",
        ),
        ConnectError::NoDroneAvailable => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "No active drone available.",
        ),
        ConnectError::FailedToRemoveKey => err_to_response(
            connect_error,
            StatusCode::CONFLICT,
            "Failed to remove lock.",
        ),
        ConnectError::Sql(_) | ConnectError::Serialization(_) => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error.",
        ),
        ConnectError::NoClusterProvided => err_to_response(
            connect_error,
            StatusCode::BAD_REQUEST,
            "No cluster provided, and no default cluster for this controller.",
        ),
        ConnectError::Other(_) => err_to_response(
            connect_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error.",
        ),
    }
}

pub async fn handle_connect(
    State(controller): State<Controller>,
    Json(request): Json<ConnectRequest>,
) -> Result<Json<ConnectResponse>, Response> {
    let response = controller
        .connect(&request)
        .await
        .map_err(|e| connect_error_to_response(&e))?;
    Ok(Json(response))
}
