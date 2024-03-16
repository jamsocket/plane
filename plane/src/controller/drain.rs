use super::{core::Controller, error::IntoApiError};
use crate::{
    names::DroneName,
    types::{ClusterName, DrainResult},
};
use axum::{
    extract::{Path, State},
    response::Response,
    Json,
};

async fn drain(
    controller: &Controller,
    cluster: &ClusterName,
    drone: &DroneName,
) -> Result<DrainResult, Response> {
    let drone_id = controller
        .db
        .node()
        .get_id(cluster, drone)
        .await
        .or_internal_error("Database error")?
        .or_not_found("Drone does not exist")?;

    println!("Draining drone with id {}", drone_id);

    let updated = controller
        .db
        .drone()
        .drain(drone_id)
        .await
        .or_internal_error("Database error")?;

    println!("Done");

    Ok(DrainResult { updated })
}

pub async fn handle_drain(
    Path((cluster, drone)): Path<(ClusterName, DroneName)>,
    State(controller): State<Controller>,
) -> Result<Json<DrainResult>, Response> {
    let result = drain(&controller, &cluster, &drone).await?;
    Ok(Json(result))
}
