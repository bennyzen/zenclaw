use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;

#[async_trait]
pub trait Runtime: Send + Sync {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>);
    async fn sleep(&self, ms: u64);
}
