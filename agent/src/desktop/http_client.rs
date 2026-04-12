use async_trait::async_trait;

use crate::platform::http_client::{Headers, HttpClient, Response};

/// Desktop HTTP client backed by reqwest.
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("Failed to build reqwest client"),
        }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(
        &self,
        url: &str,
        headers: &Headers,
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
        let mut req = self.client.get(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let resp_headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp.bytes().await?.to_vec();
        Ok(Response {
            status,
            body,
            headers: resp_headers,
        })
    }

    async fn post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
        let mut req = self.client.post(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.body(body.to_vec()).send().await?;
        let status = resp.status().as_u16();
        let resp_headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let resp_body = resp.bytes().await?.to_vec();
        Ok(Response {
            status,
            body: resp_body,
            headers: resp_headers,
        })
    }

    async fn put(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
        let mut req = self.client.put(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.body(body.to_vec()).send().await?;
        let status = resp.status().as_u16();
        let resp_headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let resp_body = resp.bytes().await?.to_vec();
        Ok(Response {
            status,
            body: resp_body,
            headers: resp_headers,
        })
    }

    async fn delete(
        &self,
        url: &str,
        headers: &Headers,
    ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
        let mut req = self.client.delete(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let resp_headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp.bytes().await?.to_vec();
        Ok(Response {
            status,
            body,
            headers: resp_headers,
        })
    }

    async fn stream_post(
        &self,
        _url: &str,
        _headers: &Headers,
        _body: &[u8],
        _on_chunk: Box<dyn FnMut(&str) + Send>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: SSE streaming with reqwest-eventsource
        Ok(())
    }
}
