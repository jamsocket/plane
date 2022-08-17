use anyhow::Result;
use dev::{resources::{nats::nats, pebble::pebble}};
use integration_test::integration_test;

#[integration_test]
async fn test_cert_refresh_inner() -> Result<()> {
    let nats = nats().await?;
    let pebble = pebble().await?;

    Ok(())
}
