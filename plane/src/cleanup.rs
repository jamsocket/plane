use crate::database::{subscribe::EventSubscriptionManager, PlaneDatabase};
use anyhow::Result;

const CLEANUP_LOOP_INTERVAL_SECONDS: u64 = 60 * 15;

pub async fn run_cleanup(
    db: &PlaneDatabase,
    min_age_days: Option<i32>,
    cleanup_batch_size: i32,
) -> Result<()> {
    tracing::info!("Running cleanup");

    if let Some(min_age_days) = min_age_days {
        db.backend()
            .cleanup(min_age_days, cleanup_batch_size)
            .await?;
        EventSubscriptionManager::clean_up_events(&db.pool, min_age_days).await?;
    }

    db.clean_up_tokens().await?;

    tracing::info!("Done running cleanup");

    Ok(())
}

pub async fn run_cleanup_loop(
    db: PlaneDatabase,
    min_age_days: Option<i32>,
    cleanup_batch_size: i32,
) {
    // Each controller runs a cleanup loop. To avoid having them all run at the same time, we
    // introduce a random offset to the start time.
    let random_offset_seconds = rand::random::<u64>() % CLEANUP_LOOP_INTERVAL_SECONDS;
    tokio::time::sleep(tokio::time::Duration::from_secs(random_offset_seconds)).await;

    loop {
        if let Err(e) = run_cleanup(&db, min_age_days, cleanup_batch_size).await {
            tracing::error!("Error running cleanup: {:?}", e);
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(
            CLEANUP_LOOP_INTERVAL_SECONDS,
        ))
        .await;
    }
}
