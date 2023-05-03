use crate::scheduler::SchedulerError;
use anyhow::anyhow;
use chrono::Utc;
use plane_core::{
    messages::{
        scheduler::{BackendResource, ImageResource, Resource, ScheduleRequest, ScheduleResponse},
        state::{
            BackendMessage, BackendMessageType, ClusterStateMessage, DroneMessage,
            DroneMessageType, ImageDownloadMessage, WorldStateMessage,
        },
    },
    nats::{MessageWithResponseHandle, TypedNats},
    state::StateHandle,
    timing::Timer,
    types::ClusterName,
    NeverResult,
};
use scheduler::Scheduler;

mod config;
pub mod dns;
pub mod drone_state;
pub mod plan;
pub mod run;
mod scheduler;

async fn schedule_spawn(
    backend: &BackendResource,
    scheduler: &Scheduler,
    cluster: &ClusterName,
    nats: &TypedNats,
) -> anyhow::Result<ScheduleResponse> {
    tracing::info!(spawn_request=?backend, "Got spawn request");
    Ok(match scheduler.schedule(cluster, Utc::now()) {
        Ok(drone_id) => {
            let timer = Timer::new();
            let spawn_request = backend.schedule(cluster, &drone_id);
            match nats.request(&spawn_request).await {
                Ok(true) => {
                    tracing::info!(
                        duration=?timer.duration(),
                        backend_id=%spawn_request.backend_id,
                        %drone_id,
                        "Drone accepted backend."
                    );

                    nats.publish(&WorldStateMessage {
                        cluster: cluster.clone(),
                        message: ClusterStateMessage::BackendMessage(BackendMessage {
                            backend: spawn_request.backend_id.clone(),
                            message: BackendMessageType::Assignment {
                                drone: drone_id.clone(),
                            },
                        }),
                    })
                    .await?;

                    ScheduleResponse::ScheduledBackend {
                        drone: drone_id,
                        backend_id: spawn_request.backend_id,
                        bearer_token: spawn_request.bearer_token.clone(),
                    }
                }
                Ok(false) => {
                    tracing::warn!("Drone rejected backend.");
                    ScheduleResponse::NoDroneAvailable
                }
                Err(error) => {
                    tracing::warn!(?error, "Scheduler returned error.");
                    ScheduleResponse::NoDroneAvailable
                }
            }
        }
        Err(error) => match error {
            SchedulerError::NoDroneAvailable => {
                tracing::warn!("No drone available.");
                ScheduleResponse::NoDroneAvailable
            }
        },
    })
}

async fn schedule_image(
    image: &ImageResource,
    scheduler: &Scheduler,
    cluster: &ClusterName,
    nats: &TypedNats,
) -> anyhow::Result<ScheduleResponse> {
    tracing::info!(image=?image, "Got Image Request");
    Ok(match scheduler.schedule(cluster, Utc::now()) {
        Ok(ref drone_id) => {
            let image_download_request = image.schedule(cluster, drone_id);
            if let true = nats.request(&image_download_request).await? {
                nats.publish(&WorldStateMessage {
                    cluster: cluster.clone(),
                    message: ClusterStateMessage::DroneMessage(DroneMessage {
                        drone: drone_id.clone(),
                        message: DroneMessageType::Image(image.url.clone()),
                    }),
                })
                .await?;

                ScheduleResponse::ScheduledImage {
                    drone: drone_id.clone(),
                    image: image.url.clone(),
                }
            } else {
                panic!("shut up")
            }
        }
        Err(SchedulerError::NoDroneAvailable) => {
            todo!();
        }
    })
}

pub async fn run_scheduler(nats: TypedNats, state: StateHandle) -> NeverResult {
    let scheduler = Scheduler::new(state);
    let mut schedule_request_sub = nats.subscribe(ScheduleRequest::subscribe_subject()).await?;
    tracing::info!("Subscribed to spawn requests.");

    while let Some(
        ref msg @ MessageWithResponseHandle {
            value:
                ScheduleRequest {
                    ref resource,
                    ref cluster,
                },
            ..
        },
    ) = schedule_request_sub.next().await
    {
        let result = match resource {
            Resource::Image(image) => schedule_image(image, &scheduler, cluster, &nats).await?,
            Resource::Backend(backend) => {
                schedule_spawn(backend, &scheduler, cluster, &nats).await?
            }
        };

        msg.respond(&result).await?;
    }

    Err(anyhow!(
        "Scheduler stream closed before pending messages read."
    ))
}
