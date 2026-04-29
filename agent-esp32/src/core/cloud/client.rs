//! Minimal S3-compatible client (Cloudflare R2 in our deployment).
//!
//! All calls are blocking and use esp-idf-svc's `EspHttpConnection` with
//! mbedTLS for HTTPS. SigV4 signing comes from [`super::sigv4`]. The
//! response surface mirrors the MicroPython implementation in
//! `firmware/lib/s3.py` so the consuming web UI can stay unchanged.

#[cfg(feature = "esp32")]
use crate::config::StorageConfig;

#[cfg(feature = "esp32")]
use chrono::Utc;
#[cfg(feature = "esp32")]
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
#[cfg(feature = "esp32")]
use esp_idf_svc::http::Method;
#[cfg(feature = "esp32")]
use std::io::Write;

#[cfg(feature = "esp32")]
use super::sigv4::{presign_url, sign_authorization_header, SignInput};

/// One object returned from a LIST call.
#[derive(Debug, Clone)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
}

/// Result of LIST: both real objects and synthetic "directories" (the
/// `<CommonPrefixes>` returned when delimiter='/' is requested).
#[derive(Debug, Clone, Default)]
pub struct S3Listing {
    pub objects: Vec<S3Object>,
    pub common_prefixes: Vec<String>,
}

/// Errors returned by [`S3Client`]. Stringly-typed for now — the only
/// consumer is the HTTP layer that maps them to JSON error fields.
#[derive(Debug)]
pub struct S3Error(pub String);

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for S3Error {}

type Result<T> = std::result::Result<T, S3Error>;

#[cfg(feature = "esp32")]
pub struct S3Client {
    /// Endpoint scheme+host with no trailing slash (e.g.
    /// `https://abcdef.r2.cloudflarestorage.com`).
    scheme_host: String,
    /// Hostname only — used for the `Host` header in SigV4.
    host: String,
    bucket: String,
    access_key: String,
    secret_key: String,
    region: String,
}

#[cfg(feature = "esp32")]
impl S3Client {
    /// Build a client from the agent's StorageConfig. Returns `None` if
    /// the config doesn't have the four required fields.
    pub fn from_config(cfg: &StorageConfig) -> Option<Self> {
        if !cfg.is_cloud_configured() {
            return None;
        }
        let endpoint = cfg.endpoint.as_deref()?.trim_end_matches('/').to_string();
        let host = endpoint
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .split('/')
            .next()?
            .to_string();
        Some(Self {
            scheme_host: endpoint,
            host,
            bucket: cfg.bucket.clone()?,
            access_key: cfg.access_key_id.clone()?,
            secret_key: cfg.secret_access_key.clone()?,
            region: cfg.region.clone(),
        })
    }

    /// `<scheme_host>/<bucket>/<key>`.
    fn object_path(&self, key: &str) -> String {
        format!("/{}/{}", self.bucket, key.trim_start_matches('/'))
    }

    fn bucket_path(&self) -> String {
        format!("/{}", self.bucket)
    }

    /// LIST objects via ListObjectsV2. Caller-supplied prefix and
    /// delimiter ("/" gives directory-style listings via CommonPrefixes).
    pub fn list(&self, prefix: &str, delimiter: Option<&str>, max_keys: u32) -> Result<S3Listing> {
        let max_keys_str = max_keys.to_string();
        let mut query: Vec<(&str, &str)> = vec![
            ("list-type", "2"),
            ("max-keys", max_keys_str.as_str()),
        ];
        if !prefix.is_empty() {
            query.push(("prefix", prefix));
        }
        if let Some(d) = delimiter {
            query.push(("delimiter", d));
        }

        let path = self.bucket_path();
        let signed = sign_authorization_header(&SignInput {
            method: "GET",
            host: &self.host,
            path: &path,
            query: &query,
            extra_headers: &[],
            body: &[],
            access_key: &self.access_key,
            secret_key: &self.secret_key,
            region: &self.region,
            service: "s3",
            now: Utc::now(),
        });

        let url = build_url(&self.scheme_host, &path, &query);
        let body = http_request(
            Method::Get,
            &url,
            &[
                ("Authorization", signed.authorization.as_str()),
                ("x-amz-date", signed.amz_date.as_str()),
                ("x-amz-content-sha256", signed.content_sha256.as_str()),
                ("Host", self.host.as_str()),
            ],
            &[],
        )?;

        Ok(parse_list_xml(&body))
    }

    /// Generate a presigned URL valid for `expires_secs` seconds.
    pub fn presign(&self, method: &str, key: &str, expires_secs: u32) -> String {
        presign_url(
            method,
            &self.scheme_host,
            &self.object_path(key),
            &self.access_key,
            &self.secret_key,
            &self.region,
            "s3",
            expires_secs,
            Utc::now(),
        )
    }
}

/// Build a full URL from scheme+host, path, and unsorted query params.
/// Query keys are emitted in input order (signing already handles
/// sorting — what we send on the wire only needs to match the original
/// canonical ordering for unsigned headers).
fn build_url(scheme_host: &str, path: &str, query: &[(&str, &str)]) -> String {
    if query.is_empty() {
        format!("{}{}", scheme_host.trim_end_matches('/'), path)
    } else {
        let qs = query
            .iter()
            .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{}{}?{}", scheme_host.trim_end_matches('/'), path, qs)
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || b == b'-'
            || b == b'_'
            || b == b'.'
            || b == b'~';
        if unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Issue an HTTPS request with the provided headers. Returns the
/// decoded UTF-8 response body. Status >= 400 maps to an error with
/// the response body included for debugging.
#[cfg(feature = "esp32")]
fn http_request(
    method: Method,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<String> {
    let config = HttpConfig {
        buffer_size: Some(2048),
        buffer_size_tx: Some(2048),
        timeout: Some(std::time::Duration::from_secs(30)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    let mut conn = EspHttpConnection::new(&config)
        .map_err(|e| S3Error(format!("HTTP init: {}", e)))?;

    let content_len = body.len().to_string();
    let mut hdrs: Vec<(&str, &str)> = headers.to_vec();
    if !body.is_empty() {
        hdrs.push(("Content-Length", &content_len));
    }

    conn.initiate_request(method, url, &hdrs)
        .map_err(|e| S3Error(format!("HTTP request: {}", e)))?;

    if !body.is_empty() {
        conn.write_all(body)
            .map_err(|e| S3Error(format!("HTTP write: {}", e)))?;
    }

    conn.initiate_response()
        .map_err(|e| S3Error(format!("HTTP response: {}", e)))?;

    let status = conn.status();

    let mut buf = [0u8; 1024];
    let mut resp = Vec::new();
    loop {
        let n = conn.read(&mut buf).map_err(|e| S3Error(format!("HTTP read: {}", e)))?;
        if n == 0 {
            break;
        }
        resp.extend_from_slice(&buf[..n]);
    }
    drop(conn);

    let body_str = String::from_utf8(resp)
        .map_err(|e| S3Error(format!("Invalid UTF-8 in response: {}", e)))?;

    if status >= 400 {
        return Err(S3Error(format!(
            "HTTP {}: {}",
            status,
            &body_str[..body_str.len().min(500)]
        )));
    }

    Ok(body_str)
}

/// Minimal XML parse for ListObjectsV2 responses. Extracts `<Key>` +
/// `<Size>` tuples from `<Contents>` blocks and `<Prefix>` strings
/// from `<CommonPrefixes>`. Mirrors the regex approach in
/// `firmware/lib/s3.py` — good enough for S3-compatible servers that
/// emit predictable output.
fn parse_list_xml(xml: &str) -> S3Listing {
    let mut listing = S3Listing::default();

    for contents in extract_blocks(xml, "<Contents>", "</Contents>") {
        let key = extract_tag(contents, "Key").unwrap_or_default();
        let size = extract_tag(contents, "Size")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if !key.is_empty() {
            listing.objects.push(S3Object { key, size });
        }
    }

    for cp in extract_blocks(xml, "<CommonPrefixes>", "</CommonPrefixes>") {
        if let Some(prefix) = extract_tag(cp, "Prefix") {
            listing.common_prefixes.push(prefix);
        }
    }

    listing
}

fn extract_blocks<'a>(haystack: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = haystack;
    while let Some(start) = rest.find(open) {
        let after_open = &rest[start + open.len()..];
        if let Some(end) = after_open.find(close) {
            out.push(&after_open[..end]);
            rest = &after_open[end + close.len()..];
        } else {
            break;
        }
    }
    out
}

fn extract_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)?;
    Some(block[start..start + end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_xml_extracts_keys_and_sizes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>bucket</Name>
  <Contents>
    <Key>foo.txt</Key>
    <Size>1024</Size>
  </Contents>
  <Contents>
    <Key>bar.bin</Key>
    <Size>4096</Size>
  </Contents>
  <CommonPrefixes>
    <Prefix>subdir/</Prefix>
  </CommonPrefixes>
</ListBucketResult>"#;
        let listing = parse_list_xml(xml);
        assert_eq!(listing.objects.len(), 2);
        assert_eq!(listing.objects[0].key, "foo.txt");
        assert_eq!(listing.objects[0].size, 1024);
        assert_eq!(listing.objects[1].key, "bar.bin");
        assert_eq!(listing.objects[1].size, 4096);
        assert_eq!(listing.common_prefixes, vec!["subdir/".to_string()]);
    }

    #[test]
    fn parse_list_xml_handles_empty_listing() {
        let xml = r#"<ListBucketResult><Name>bucket</Name></ListBucketResult>"#;
        let listing = parse_list_xml(xml);
        assert!(listing.objects.is_empty());
        assert!(listing.common_prefixes.is_empty());
    }

    #[test]
    fn build_url_omits_question_mark_for_empty_query() {
        assert_eq!(
            build_url("https://example.r2.cloudflarestorage.com", "/bucket/key", &[]),
            "https://example.r2.cloudflarestorage.com/bucket/key"
        );
    }

    #[test]
    fn build_url_encodes_query_values() {
        let url = build_url(
            "https://example.r2.cloudflarestorage.com",
            "/bucket",
            &[("prefix", "folder/sub item")],
        );
        assert_eq!(
            url,
            "https://example.r2.cloudflarestorage.com/bucket?prefix=folder%2Fsub%20item"
        );
    }
}
