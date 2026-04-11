use async_trait::async_trait;
use std::collections::HashMap;

pub type Headers = HashMap<String, String>;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Headers,
}

#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get(
        &self,
        url: &str,
        headers: &Headers,
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>>;

    async fn post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>>;

    async fn put(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>>;

    async fn delete(
        &self,
        url: &str,
        headers: &Headers,
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>>;

    async fn stream_post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
        on_chunk: Box<dyn FnMut(&str) + Send>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
