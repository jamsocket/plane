use anyhow::Result;
use dev::{system::nats, TEARDOWN_TASK_MANAGER};
use integration_test::integration_test;

#[integration_test]
async fn test_cert_refresh_inner() -> Result<()> {
    let _c = nats().await?;

    Ok(()) as anyhow::Result<_>
}
