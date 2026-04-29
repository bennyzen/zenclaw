//! AWS Signature Version 4 — minimal hand-rolled implementation for
//! signing requests to S3-compatible endpoints (Cloudflare R2 in our case).
//!
//! Reference: <https://docs.aws.amazon.com/general/latest/gr/sigv4-signed-request-examples.html>
//!
//! Two entry points:
//! - [`sign_authorization_header`] for authenticated direct calls (LIST,
//!   PUT, DELETE, HEAD) where we attach an `Authorization:` header.
//! - [`presign_url`] for browser-friendly URLs (GET/PUT/DELETE) where the
//!   signature lives in `X-Amz-*` query params.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const TERMINATOR: &str = "aws4_request";

/// Hex-encode `bytes` to lowercase. A few-line helper avoids pulling the
/// `hex` crate just for this.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Percent-encode per RFC 3986 unreserved set (`A–Z a–z 0–9 - _ . ~`).
/// When `keep_slash` is true, `/` is passed through — matching the AWS
/// canonical-URI rule for object keys.
fn percent_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || b == b'-'
            || b == b'_'
            || b == b'.'
            || b == b'~';
        if unreserved || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length is unrestricted");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex_encode(&Sha256::digest(data))
}

/// Build the SigV4 signing key for a (date, region, service) triple.
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{}", secret).into_bytes();
    let k_date = hmac_sha256(&k_secret, date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, TERMINATOR.as_bytes())
}

/// Format a UTC instant as `YYYYMMDDTHHMMSSZ` and the date-only form
/// `YYYYMMDD`, both required by SigV4.
fn format_dates(now: DateTime<Utc>) -> (String, String) {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    (amz_date, date_stamp)
}

/// Build the canonical query string by sorting parameters by name (and
/// then by value for equal names) and percent-encoding both sides.
fn canonical_query(params: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (percent_encode(k, false), percent_encode(v, false)))
        .collect();
    sorted.sort();
    sorted
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
}

/// Build the canonical headers block plus the matching signed-headers
/// list. Header names are lowercased; values are trimmed.
fn canonical_headers(headers: &[(&str, &str)]) -> (String, String) {
    let mut sorted: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical = sorted
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k, v))
        .collect::<String>();
    let signed = sorted
        .iter()
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>()
        .join(";");
    (canonical, signed)
}

/// Inputs needed to sign a request. `path` is the URL path starting with
/// `/`; `host` is the request hostname (and port if non-default).
pub struct SignInput<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub query: &'a [(&'a str, &'a str)],
    pub extra_headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub now: DateTime<Utc>,
}

/// Output of [`sign_authorization_header`]: the headers to attach to the
/// request, including `Authorization`. Caller appends these to whatever
/// HTTP client they use.
pub struct SignedHeaders {
    pub authorization: String,
    pub amz_date: String,
    pub content_sha256: String,
}

/// Sign a request with an `Authorization` header. The caller must send
/// the headers (`Host` is implicit from the URL; `x-amz-date` and
/// `x-amz-content-sha256` come back in the result).
pub fn sign_authorization_header(input: &SignInput) -> SignedHeaders {
    let (amz_date, date_stamp) = format_dates(input.now);
    let payload_hash = sha256_hex(input.body);

    let canonical_uri = percent_encode(input.path, true);
    let canonical_qs = canonical_query(input.query);

    // Required headers for SigV4: host, x-amz-content-sha256, x-amz-date.
    // Caller-supplied extras (e.g. content-type) are merged in.
    let mut headers: Vec<(&str, &str)> = vec![
        ("host", input.host),
        ("x-amz-content-sha256", payload_hash.as_str()),
        ("x-amz-date", amz_date.as_str()),
    ];
    headers.extend_from_slice(input.extra_headers);
    let (canon_headers, signed_headers) = canonical_headers(&headers);

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method,
        canonical_uri,
        canonical_qs,
        canon_headers,
        signed_headers,
        payload_hash,
    );

    let credential_scope = format!(
        "{}/{}/{}/{}",
        date_stamp, input.region, input.service, TERMINATOR
    );
    let string_to_sign = format!(
        "{}\n{}\n{}\n{}",
        ALGORITHM,
        amz_date,
        credential_scope,
        sha256_hex(canonical_request.as_bytes()),
    );

    let key = signing_key(input.secret_key, &date_stamp, input.region, input.service);
    let signature = hex_encode(&hmac_sha256(&key, string_to_sign.as_bytes()));

    let authorization = format!(
        "{} Credential={}/{}, SignedHeaders={}, Signature={}",
        ALGORITHM, input.access_key, credential_scope, signed_headers, signature
    );

    SignedHeaders {
        authorization,
        amz_date,
        content_sha256: payload_hash,
    }
}

/// Generate a presigned URL valid for `expires_secs` seconds. The
/// signature lives in `X-Amz-Signature`; payload is unsigned (the
/// browser uploads/downloads bytes without the device hashing them).
pub fn presign_url(
    method: &str,
    scheme_host: &str,
    path: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
    expires_secs: u32,
    now: DateTime<Utc>,
) -> String {
    let (amz_date, date_stamp) = format_dates(now);
    let credential = format!(
        "{}/{}/{}/{}/{}",
        access_key, date_stamp, region, service, TERMINATOR
    );

    // Host header is the only signed header for presigned URLs.
    let host = scheme_host
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("");

    let signed_headers = "host";
    let payload_hash = "UNSIGNED-PAYLOAD";

    let expires_str = expires_secs.to_string();
    let query: Vec<(&str, &str)> = vec![
        ("X-Amz-Algorithm", ALGORITHM),
        ("X-Amz-Credential", credential.as_str()),
        ("X-Amz-Date", amz_date.as_str()),
        ("X-Amz-Expires", expires_str.as_str()),
        ("X-Amz-SignedHeaders", signed_headers),
    ];

    let canonical_uri = percent_encode(path, true);
    let canonical_qs = canonical_query(&query);
    let canon_headers = format!("host:{}\n", host);

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method, canonical_uri, canonical_qs, canon_headers, signed_headers, payload_hash,
    );

    let credential_scope = format!(
        "{}/{}/{}/{}",
        date_stamp, region, service, TERMINATOR
    );
    let string_to_sign = format!(
        "{}\n{}\n{}\n{}",
        ALGORITHM,
        amz_date,
        credential_scope,
        sha256_hex(canonical_request.as_bytes()),
    );

    let key = signing_key(secret_key, &date_stamp, region, service);
    let signature = hex_encode(&hmac_sha256(&key, string_to_sign.as_bytes()));

    format!(
        "{}{}?{}&X-Amz-Signature={}",
        scheme_host.trim_end_matches('/'),
        canonical_uri,
        canonical_qs,
        signature,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn percent_encoding_keeps_slash_when_asked() {
        assert_eq!(percent_encode("/foo/bar baz", true), "/foo/bar%20baz");
        assert_eq!(percent_encode("/foo/bar baz", false), "%2Ffoo%2Fbar%20baz");
    }

    #[test]
    fn percent_encoding_unreserved_passthrough() {
        assert_eq!(percent_encode("Aa0-_.~", false), "Aa0-_.~");
    }

    #[test]
    fn signing_key_matches_aws_published_vector() {
        // Published reference: aws-go-sdk and AWS docs both list this
        // exact (date=20120215, region=us-east-1, service=iam) example.
        let key = signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex_encode(&key),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d",
        );
    }

    #[test]
    fn authorization_header_has_correct_structure() {
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let input = SignInput {
            method: "GET",
            host: "examplebucket.s3.amazonaws.com",
            path: "/test.txt",
            query: &[],
            extra_headers: &[("range", "bytes=0-9")],
            body: &[],
            access_key: "AKIAIOSFODNN7EXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            service: "s3",
            now,
        };
        let signed = sign_authorization_header(&input);

        // amz-date is the canonical SigV4 timestamp.
        assert_eq!(signed.amz_date, "20130524T000000Z");
        // SHA256 of empty body — well-known constant.
        assert_eq!(
            signed.content_sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );

        let auth = &signed.authorization;
        assert!(auth.starts_with("AWS4-HMAC-SHA256 "));
        assert!(auth.contains("Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request"));
        // Signed headers must be sorted lowercase, semicolon-joined, and
        // include the three required SigV4 headers plus the caller-supplied
        // `range`.
        assert!(auth.contains("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date"));
        // Signature is 64 hex chars (SHA-256 → 32 bytes → 64 hex).
        let sig = auth.rsplit_once("Signature=").unwrap().1;
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn signature_changes_when_inputs_change() {
        // Cheap sanity check that the signer is actually consuming all
        // inputs — flipping the method alone must change the signature.
        let now = Utc.with_ymd_and_hms(2026, 4, 29, 0, 0, 0).unwrap();
        let mut input = SignInput {
            method: "GET",
            host: "bucket.r2.cloudflarestorage.com",
            path: "/foo.txt",
            query: &[],
            extra_headers: &[],
            body: &[],
            access_key: "AKID",
            secret_key: "secret",
            region: "auto",
            service: "s3",
            now,
        };
        let a = sign_authorization_header(&input).authorization;
        input.method = "PUT";
        let b = sign_authorization_header(&input).authorization;
        assert_ne!(a, b);
    }

    #[test]
    fn presign_url_contains_required_query_params() {
        let now = Utc.with_ymd_and_hms(2026, 4, 29, 12, 0, 0).unwrap();
        let url = presign_url(
            "GET",
            "https://example.r2.cloudflarestorage.com/bucket",
            "/foo/bar.txt",
            "AKID",
            "secret",
            "auto",
            "s3",
            900,
            now,
        );
        assert!(url.starts_with("https://example.r2.cloudflarestorage.com/bucket/foo/bar.txt?"));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("X-Amz-Expires=900"));
        assert!(url.contains("X-Amz-SignedHeaders=host"));
        assert!(url.contains("X-Amz-Signature="));
    }
}
