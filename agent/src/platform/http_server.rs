use async_trait::async_trait;

#[async_trait]
pub trait HttpServer: Send + Sync {
    async fn start(&self, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
