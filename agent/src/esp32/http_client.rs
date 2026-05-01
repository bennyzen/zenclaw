//! HttpClient implementation backed by esp-idf-svc's blocking HTTP client
//! with TLS via the bundled certificate store.
//!
//! Internally takes `crate::TLS_MUTEX` per call — the device can only sustain
//! one mbedTLS context at a time, so concurrent HTTPS calls would otherwise
//! corrupt each other. Held for the full duration of one request (handshake
//! + body). On poison, recovers via `into_inner` since there's no way to
//! reset mbedTLS without rebooting the chip.
//!
//! The `async fn` bodies execute synchronously when polled — ESP32 has no
//! executor that yields on I/O. This is the same pattern used in
//! `esp32/runner.rs` and works under `block_on`.

use async_trait::async_trait;
use embedded_svc::io::Write as _;
use std::time::Duration;

use crate::platform::http_client::{Headers, HttpClient, Response};

pub struct EspHttpClient {
    timeout: Duration,
}

impl EspHttpClient {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

impl Default for EspHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
impl HttpClient for EspHttpClient {
    async fn get(&self, url: &str, headers: &Headers) -> Result<Response, BoxErr> {
        execute(self, esp_idf_svc::http::Method::Get, url, headers, None)
    }

    async fn post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, BoxErr> {
        execute(
            self,
            esp_idf_svc::http::Method::Post,
            url,
            headers,
            Some(body),
        )
    }

    async fn put(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, BoxErr> {
        execute(
            self,
            esp_idf_svc::http::Method::Put,
            url,
            headers,
            Some(body),
        )
    }

    async fn delete(&self, url: &str, headers: &Headers) -> Result<Response, BoxErr> {
        execute(self, esp_idf_svc::http::Method::Delete, url, headers, None)
    }

    async fn stream_post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
        mut on_chunk: Box<dyn FnMut(String) + Send>,
    ) -> Result<(), BoxErr> {
        use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

        let _tls_guard = crate::TLS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let config = HttpConfig {
            buffer_size: Some(1024),
            buffer_size_tx: Some(1024),
            timeout: Some(self.timeout),
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let mut conn =
            EspHttpConnection::new(&config).map_err(|e| format!("HTTP init: {}", e))?;

        let body_len = body.len().to_string();
        let mut header_pairs: Vec<(&str, &str)> = Vec::new();
        header_pairs.push(("Content-Length", body_len.as_str()));
        for (k, v) in headers {
            header_pairs.push((k.as_str(), v.as_str()));
        }

        conn.initiate_request(esp_idf_svc::http::Method::Post, url, &header_pairs)
            .map_err(|e| format!("req: {}", e))?;
        conn.write_all(body).map_err(|e| format!("write: {}", e))?;
        conn.initiate_response()
            .map_err(|e| format!("resp: {}", e))?;

        let mut buf = [0u8; 2048];
        loop {
            let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
            if n == 0 {
                break;
            }
            // Best-effort UTF-8 decode of this chunk; SSE/text streams should be valid.
            let s = std::str::from_utf8(&buf[..n])
                .map(String::from)
                .unwrap_or_else(|_| String::from_utf8_lossy(&buf[..n]).into_owned());
            on_chunk(s);
        }
        Ok(())
    }
}

fn execute(
    client: &EspHttpClient,
    method: esp_idf_svc::http::Method,
    url: &str,
    headers: &Headers,
    body: Option<&[u8]>,
) -> Result<Response, BoxErr> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

    let _tls_guard = crate::TLS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    let config = HttpConfig {
        buffer_size: Some(1024),
        buffer_size_tx: Some(1024),
        timeout: Some(client.timeout),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut conn =
        EspHttpConnection::new(&config).map_err(|e| format!("HTTP init: {}", e))?;

    let body_len_str;
    let mut header_pairs: Vec<(&str, &str)> = Vec::new();
    if let Some(b) = body {
        body_len_str = b.len().to_string();
        header_pairs.push(("Content-Length", body_len_str.as_str()));
    }
    for (k, v) in headers {
        header_pairs.push((k.as_str(), v.as_str()));
    }

    conn.initiate_request(method, url, &header_pairs)
        .map_err(|e| format!("req: {}", e))?;

    if let Some(b) = body {
        conn.write_all(b).map_err(|e| format!("write: {}", e))?;
    }

    conn.initiate_response()
        .map_err(|e| format!("resp: {}", e))?;

    let status = conn.status();
    let mut resp_body = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 {
            break;
        }
        resp_body.extend_from_slice(&buf[..n]);
    }

    Ok(Response {
        status,
        body: resp_body,
        headers: Headers::new(),
    })
}
