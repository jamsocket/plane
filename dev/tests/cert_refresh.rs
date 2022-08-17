use std::{future::Future, time::Duration};

use anyhow::Result;
use dev::resources::{nats::nats, pebble::pebble};
use dis_spawner::messages::cert::SetAcmeDnsRecord;
use integration_test::integration_test;
use tokio::time::error::Elapsed;

#[must_use]
fn spawn_timeout<F>(timeout_ms: u64, future: F) -> tokio::task::JoinHandle<std::result::Result<Result<()>, Elapsed>> where F: Future<Output=Result<()>> + Send + Sync + 'static {
    tokio::spawn(tokio::time::timeout(Duration::from_millis(timeout_ms), future))
}

#[integration_test]
async fn test_cert_refresh() -> Result<()> {
    let nats = nats().await?;
    let pebble = pebble().await?;
    let conn = nats.connection().await?;

    let mut dns_sub = conn.subscribe(SetAcmeDnsRecord::subject()).await?;
    let handle = spawn_timeout(1000, async move {
        let message = dns_sub.next().await.unwrap().unwrap();
        assert_eq!("mydomain.test", message.value.cluster);

        message.respond(&true).await?;

        Ok(())
    });

    let result = dis_spawner_drone::drone::cert::get_certificate(
        "spawner.test",
        &conn,
        &pebble.directory_url(),
        "admin@spawner.test",
        &pebble.client()?,
        None,
    ).await?;

    handle.await???;

    Ok(())
}
