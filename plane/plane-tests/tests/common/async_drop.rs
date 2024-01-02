use anyhow::Result;

#[async_trait::async_trait]
pub trait AsyncDrop {
    async fn drop_future(&self) -> Result<()>;
}
