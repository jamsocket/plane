use crate::util::random_string;
use axum::{
    response::{IntoResponse, Response},
    Json,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{Debug, Display},
};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum ApiErrorKind {
    FailedToAcquireKey,
    KeyUnheldNoSpawnConfig,
    KeyHeldUnhealthy,
    KeyHeld,
    NoDroneAvailable,
    FailedToRemoveKey,
    DatabaseError,
    NoClusterProvided,
    NotFound,
    InvalidClusterName,
    Other,
}

#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub id: String,
    pub kind: ApiErrorKind,
    pub message: String,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn err_to_response<E: Debug>(
    error: E,
    status: StatusCode,
    user_message: &str,
    code: ApiErrorKind,
) -> Response {
    let err_id = random_string();

    if status.is_server_error() {
        tracing::error!(
            ?err_id,
            ?error,
            "Server error encountered while handling request."
        );
    } else {
        tracing::warn!(
            ?err_id,
            ?error,
            "Client error encountered while handling request."
        );
    }

    let result = ApiError {
        id: err_id.clone(),
        message: user_message.to_string(),
        kind: code,
    };

    (status, Json(result)).into_response()
}

pub trait IntoApiError<T>: Sized {
    fn or_not_found(self, user_message: &str) -> Result<T, Response> {
        self.or_status(StatusCode::NOT_FOUND, user_message, ApiErrorKind::NotFound)
    }

    fn or_internal_error(self, user_message: &str) -> Result<T, Response> {
        self.or_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            user_message,
            ApiErrorKind::Other,
        )
    }

    fn or_status(
        self,
        status: StatusCode,
        user_message: &str,
        code: ApiErrorKind,
    ) -> Result<T, Response>;
}

impl<T, E: Error> IntoApiError<T> for Result<T, E> {
    fn or_status(
        self,
        status: StatusCode,
        user_message: &str,
        code: ApiErrorKind,
    ) -> Result<T, Response> {
        match self {
            Ok(v) => Ok(v),
            Err(error) => {
                let err = err_to_response(&error, status, user_message, code);
                Err(err)
            }
        }
    }
}

impl<T> IntoApiError<T> for Option<T> {
    fn or_status(
        self,
        status: StatusCode,
        user_message: &str,
        code: ApiErrorKind,
    ) -> Result<T, Response> {
        match self {
            Some(v) => Ok(v),
            None => {
                let err = err_to_response("Missing value.", status, user_message, code);
                Err(err)
            }
        }
    }
}
