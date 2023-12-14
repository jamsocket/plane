use super::{core::Controller, error::IntoApiError};
use crate::{names::DroneName, types::ClusterId};
use axum::{
    extract::{Path, State},
    response::Response,
    Json,
};

async fn drain(
    controller: &Controller,
    cluster: &ClusterId,
    drone: &DroneName,
) -> Result<(), Response> {
    let drone_id = controller
        .db
        .node()
        .get_id(cluster, drone)
        .await
        .or_internal_error("Database error")?
        .or_not_found("Drone does not exist")?;

    println!("Draining drone with id {}", drone_id);

    controller
        .db
        .drone()
        .drain(drone_id)
        .await
        .or_internal_error("Database error")?;

    println!("Done");

    Ok(())
}

pub async fn handle_drain(
    Path((cluster_id, drone)): Path<(ClusterId, DroneName)>,
    State(controller): State<Controller>,
) -> Result<Json<()>, Response> {
    drain(&controller, &cluster_id, &drone).await?;
    Ok(Json(()))
}
