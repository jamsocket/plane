use crate::database::backend::BackendRow;

use super::{core::Controller, error::IntoApiError};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive},
        Response, Sse,
    },
    Json,
};
use futures_util::{Stream, StreamExt};
use plane_common::{
    names::BackendName,
    types::{backend_state::BackendStatusStreamEntry, BackendStatus},
};
use std::convert::Infallible;

impl From<BackendRow> for BackendStatusStreamEntry {
    fn from(row: BackendRow) -> Self {
        Self::from_state(row.state, row.last_status_time)
    }
}

async fn backend_status(
    controller: &Controller,
    backend_id: &BackendName,
) -> Result<BackendStatusStreamEntry, Response> {
    let backend = controller
        .db
        .backend()
        .backend(backend_id)
        .await
        .or_internal_error("Database error")?
        .or_not_found("Backend does not exist")?;

    Ok(backend.into())
}

pub async fn handle_backend_status(
    Path(backend_id): Path<BackendName>,
    State(controller): State<Controller>,
) -> Result<Json<BackendStatusStreamEntry>, Response> {
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
