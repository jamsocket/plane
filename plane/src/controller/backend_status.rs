use super::{core::Controller, error::IntoApiError};
use crate::{
    log_types::LoggableTime,
    names::BackendName,
    types::{backend_state::TimestampedBackendStatus, BackendStatus},
};
use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive},
        Response, Sse,
    },
    Json,
};
use futures_util::{Stream, StreamExt};
use hyper::HeaderMap;
use std::convert::Infallible;

async fn backend_status(
    controller: &Controller,
    backend_id: &BackendName,
) -> Result<TimestampedBackendStatus, Response> {
    let backend = controller
        .db
        .backend()
        .backend(backend_id)
        .await
        .or_internal_error("Database error")?
        .or_not_found("Backend does not exist")?;

    let result = TimestampedBackendStatus {
        status: backend.last_status,
        time: LoggableTime(backend.last_status_time),
    };

    Ok(result)
}

pub async fn handle_backend_status(
    Path(backend_id): Path<BackendName>,
    State(controller): State<Controller>,
) -> Result<Json<TimestampedBackendStatus>, Response> {
    let status = backend_status(&controller, &backend_id).await?;
    Ok(Json(status))
}

pub async fn handle_backend_status_stream(
    Path(backend_id): Path<BackendName>,
    State(controller): State<Controller>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Response> {
    let last_status: Option<BackendStatus> = headers
        .get("Last-Event-ID")
        .and_then(|id| id.to_str().ok())
        .and_then(|id| BackendStatus::try_from(id.to_owned()).ok());

    let mut st = Box::pin(
        controller
            .db
            .backend()
            .status_stream(&backend_id)
            .await
            .or_internal_error("Database error")?,
    );

    let stream = async_stream::try_stream! {
        while let Some(status) = st.next().await {
            if let Some(last_status) = last_status {
                if status.status <= last_status {
                    continue;
                }
            }

            let id = status.status.to_string();
            yield Event::default().json_data(&status).expect("always serializable").id(id);
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
