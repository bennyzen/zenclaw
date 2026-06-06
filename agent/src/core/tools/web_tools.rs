use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

// Sizing caps. Responses are soft-capped (overridable per call via
// max_length) and refused-not-truncated when exceeded. The request body
// has a hard cap so a runaway POST can't pin heap or hang a slow TLS
// upload. The real ceiling on both sides is PSRAM + TLS overhead, not a
// fixed byte count — these defaults bound heap/latency, not capability.
#[cfg(feature = "desktop")]
const DEFAULT_MAX_RESPONSE: usize = 10 * 1024 * 1024;
#[cfg(feature = "esp32")]
const DEFAULT_MAX_RESPONSE: usize = 64 * 1024; // PSRAM has room; 2x the old 32k
#[cfg(not(any(feature = "desktop", feature = "esp32")))]
const DEFAULT_MAX_RESPONSE: usize = 8 * 1024;

/// Hard cap on the request body the agent may send in one call.
#[cfg(any(feature = "desktop", feature = "esp32"))]
const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// HTTP verb, parsed from the tool arg independently of the platform HTTP
/// client so it can be unit-tested on any target.
#[cfg(any(feature = "desktop", feature = "esp32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
}

#[cfg(any(feature = "desktop", feature = "esp32"))]
impl HttpMethod {
    fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            "HEAD" => Ok(Self::Head),
            other => Err(format!(
                "web(request): unsupported method '{}'. Use GET, POST, PUT, PATCH, DELETE, or HEAD.",
                other
            )),
        }
    }

    /// Verbs that carry a request body. HEAD/GET bodies are dropped.
    fn allows_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch | Self::Delete)
    }
}

#[cfg(feature = "desktop")]
impl HttpMethod {
    fn to_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
            Self::Head => reqwest::Method::HEAD,
        }
    }
}

#[cfg(feature = "esp32")]
impl HttpMethod {
    fn to_esp(self) -> esp_idf_svc::http::Method {
        use esp_idf_svc::http::Method;
        match self {
            Self::Get => Method::Get,
            Self::Post => Method::Post,
            Self::Put => Method::Put,
            Self::Patch => Method::Patch,
            Self::Delete => Method::Delete,
            Self::Head => Method::Head,
        }
    }
}

/// Parse the optional `headers` arg (a JSON object of string→string) into
/// owned pairs. Non-string values are rejected with an actionable error
/// rather than silently coerced. The agent typically fills an auth header
/// here from a key it recalled out of memory.
#[cfg(any(feature = "desktop", feature = "esp32"))]
fn parse_headers(args: &serde_json::Value) -> Result<Vec<(String, String)>, String> {
    let raw = match args.get("headers") {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => {
            return Err("web(request): 'headers' must be an object of string → string".into())
        }
    };
    let mut out = Vec::with_capacity(raw.len());
    for (k, v) in raw {
        match v.as_str() {
            Some(s) => out.push((k.clone(), s.to_string())),
            None => {
                return Err(format!(
                    "web(request): header '{}' must have a string value",
                    k
                ))
            }
        }
    }
    Ok(out)
}

#[cfg(feature = "esp32")]
fn refusal_message(cap: usize) -> String {
    format!(
        "Response body exceeded the {}-byte fetch cap. Re-run with a larger \
         max_length, target a more specific URL, or fetch a sub-range.",
        cap,
    )
}

/// ESP32 HTTP request. Generalizes the old GET-only fetch: any verb,
/// optional headers, optional body. Returns the response body if it fits
/// in `max_length` bytes, otherwise an actionable refusal as the body
/// (with the original HTTP status preserved). Never truncates.
#[cfg(feature = "esp32")]
fn esp_http_request(
    method: HttpMethod,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
    max_length: usize,
) -> Result<(u16, String), String> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    use std::io::{Read, Write};

    let config = HttpConfig {
        buffer_size: Some(4096),
        buffer_size_tx: Some(1024),
        timeout: Some(std::time::Duration::from_secs(15)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut conn = EspHttpConnection::new(&config).map_err(|e| format!("HTTP: {}", e))?;

    // Build the header ref-slice the IDF client wants. content_len must
    // outlive the slice, so it lives in this scope.
    let content_len = body.len().to_string();
    let mut hdrs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if !body.is_empty() {
        hdrs.push(("Content-Length", &content_len));
    }

    conn.initiate_request(method.to_esp(), url, &hdrs)
        .map_err(|e| format!("req: {}", e))?;

    // Large bodies are sent by looping write (the 1024-byte TX buffer is a
    // chunk size, not a ceiling); write_all handles the chunking for us.
    if !body.is_empty() {
        conn.write_all(body).map_err(|e| format!("write: {}", e))?;
    }

    conn.initiate_response().map_err(|e| format!("resp: {}", e))?;
    let status = conn.status();

    let mut buf = [0u8; 4096];
    let mut out: Vec<u8> = Vec::with_capacity(8192);
    let mut overflowed = false;
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 {
            break;
        }
        // Memory bound: stop reading once max_length would be exceeded so
        // we don't fill heap with bytes we're going to refuse anyway.
        if out.len() + n > max_length {
            overflowed = true;
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }

    if overflowed {
        return Ok((status, refusal_message(max_length)));
    }
    Ok((status, String::from_utf8_lossy(&out).into_owned()))
}

pub struct WebTool;

#[async_trait]
impl Tool for WebTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web".to_string(),
            description: "Web access. Actions:\n\
                - fetch: GET content from a URL. Oversized bodies are refused with an actionable error rather than truncated.\n\
                - request: full HTTP request to any API — GET/POST/PUT/PATCH/DELETE/HEAD with optional headers and body. \
                  For an API that needs authentication, recall the API key from memory and pass it in `headers` \
                  (e.g. {\"Authorization\": \"Bearer <key>\"}); if you don't have a key for the API, ask the user for it and save it to memory first.\n\
                - search: web search via Brave Search API. Requires search.brave_api_key in config.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["fetch", "request", "search"],
                        "description": "Operation to perform"
                    },
                    "url":        { "type": "string",  "description": "URL to fetch/request (action=fetch|request)." },
                    "method":     { "type": "string",  "description": "HTTP verb for action=request: GET, POST, PUT, PATCH, DELETE, HEAD. Default GET." },
                    "headers":    { "type": "object",  "description": "Request headers as a string→string object (action=request). Put auth here, e.g. {\"Authorization\":\"Bearer ...\"}." },
                    "body":       { "type": "string",  "description": "Request body for write verbs (action=request). Raw string: JSON, form-encoded, etc. Set Content-Type via headers." },
                    "max_length": { "type": "integer", "description": "Maximum response body bytes (fetch/request). Oversized bodies are refused, not truncated. Default suits typical pages on the current platform." },
                    "query":      { "type": "string",  "description": "Search query (action=search)." },
                    "count":      { "type": "integer", "description": "Number of search results (search; default 5, max 20)." }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        match action {
            "fetch" => do_request(&args, HttpMethod::Get).await,
            "request" => {
                let method = match HttpMethod::parse(args["method"].as_str().unwrap_or("GET")) {
                    Ok(m) => m,
                    Err(e) => return ToolResult::Error(e),
                };
                do_request(&args, method).await
            }
            "search" => do_search(&args, ctx).await,
            "" => ToolResult::Error("web: 'action' is required".into()),
            other => ToolResult::Error(format!("web: unknown action '{}'", other)),
        }
    }
}

// --- per-action implementations ---

/// Shared GET/fetch + request path. `fetch` enters here with method=GET and
/// no body; `request` supplies the parsed method.
#[cfg_attr(not(any(feature = "desktop", feature = "esp32")), allow(unused_variables))]
async fn do_request(args: &serde_json::Value, method: HttpMethod) -> ToolResult {
    let url = match args["url"].as_str() {
        Some(u) => u,
        None => return ToolResult::Error("web: 'url' is required".into()),
    };

    let max_length = args["max_length"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_RESPONSE as u64) as usize;

    #[cfg(any(feature = "desktop", feature = "esp32"))]
    {
        let headers = match parse_headers(args) {
            Ok(h) => h,
            Err(e) => return ToolResult::Error(e),
        };

        // Body only applies to write verbs; ignore it for GET/HEAD.
        let body: Vec<u8> = if method.allows_body() {
            args["body"].as_str().unwrap_or("").as_bytes().to_vec()
        } else {
            Vec::new()
        };
        if body.len() > MAX_REQUEST_BYTES {
            return ToolResult::Error(format!(
                "web(request): request body is {} bytes, exceeds the {}-byte cap. \
                 Send less data or split the request.",
                body.len(),
                MAX_REQUEST_BYTES,
            ));
        }

        #[cfg(feature = "desktop")]
        {
            let client = reqwest::Client::new();
            let mut req = client.request(method.to_reqwest(), url);
            for (k, v) in &headers {
                req = req.header(k, v);
            }
            if !body.is_empty() {
                req = req.body(body);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    match resp.bytes().await {
                        Ok(bytes) => {
                            if bytes.len() > max_length {
                                return ToolResult::Text(format!(
                                    "HTTP {} — {}",
                                    status,
                                    refusal_message_desktop(bytes.len(), max_length),
                                ));
                            }
                            let body = String::from_utf8_lossy(&bytes).into_owned();
                            ToolResult::Text(format!("HTTP {} — {}", status, body))
                        }
                        Err(e) => ToolResult::Error(format!("Failed to read body: {}", e)),
                    }
                }
                Err(e) => ToolResult::Error(format!("Request failed: {}", e)),
            }
        }

        #[cfg(feature = "esp32")]
        {
            match esp_http_request(method, url, &headers, &body, max_length) {
                Ok((status, body)) => ToolResult::Text(format!("HTTP {} — {}", status, body)),
                Err(e) => ToolResult::Error(format!("Request failed: {}", e)),
            }
        }
    }

    #[cfg(not(any(feature = "desktop", feature = "esp32")))]
    {
        ToolResult::Error("web request not available on this platform".into())
    }
}

/// Reuses the ESP32 refusal phrasing on desktop, with the actual size
/// included so the LLM can decide how much to ask for next time.
#[cfg(feature = "desktop")]
fn refusal_message_desktop(actual: usize, cap: usize) -> String {
    format!(
        "Response body was {} bytes, exceeded the {}-byte fetch cap. Re-run \
         with a larger max_length, target a more specific URL, or fetch a \
         sub-range.",
        actual, cap,
    )
}

async fn do_search(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let query = match args["query"].as_str() {
        Some(q) if !q.is_empty() => q,
        _ => return ToolResult::Error("web(search): 'query' is required".into()),
    };
    let count = args["count"].as_u64().unwrap_or(5).min(20) as usize;

    let api_key = ctx.config.search.brave_api_key.clone();

    #[cfg(feature = "desktop")]
    {
        let api_key = match api_key {
            Some(k) if !k.is_empty() => k,
            _ => {
                return ToolResult::Text(
                    "Web search not configured. Set search.brave_api_key in config. \
                     You can still use web(action=fetch) to read URLs, or rely on Gemini's built-in Google Search grounding."
                        .to_string(),
                )
            }
        };

        let encoded = urlencode_query(query);

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            encoded, count
        );

        let client = reqwest::Client::new();
        let resp = match client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &api_key)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::Error(format!("Search failed: {}", e)),
        };

        if !resp.status().is_success() {
            return ToolResult::Error(format!("Search API HTTP {}", resp.status().as_u16()));
        }

        let body: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(e) => return ToolResult::Error(format!("Parse error: {}", e)),
        };

        format_brave_results(&body, query, count)
    }

    #[cfg(feature = "esp32")]
    {
        let api_key = match api_key {
            Some(k) if !k.is_empty() => k,
            _ => {
                return ToolResult::Text(
                    "Web search not configured. Set search.brave_api_key in config."
                        .to_string(),
                )
            }
        };

        let encoded = urlencode_query(query);

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            encoded, count
        );

        // Brave responses are typically 5-30KB; 64KB leaves headroom
        // for verbose result sets without forcing a refusal.
        let hdrs = [
            ("Accept".to_string(), "application/json".to_string()),
            ("X-Subscription-Token".to_string(), api_key),
        ];
        match esp_http_request(HttpMethod::Get, &url, &hdrs, &[], 64 * 1024) {
            Ok((_status, body)) => {
                let data: serde_json::Value = match serde_json::from_str(&body) {
                    Ok(j) => j,
                    Err(e) => return ToolResult::Error(format!("Parse error: {}", e)),
                };
                format_brave_results(&data, query, count)
            }
            Err(e) => ToolResult::Error(format!("Search failed: {}", e)),
        }
    }

    #[cfg(not(any(feature = "desktop", feature = "esp32")))]
    {
        let _ = (query, count, api_key);
        ToolResult::Error("web(search) not available on this platform".into())
    }
}

#[cfg(any(feature = "desktop", feature = "esp32"))]
fn urlencode_query(q: &str) -> String {
    q.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            _ => {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf);
                buf[..c.len_utf8()]
                    .iter()
                    .map(|b| format!("%{:02X}", b))
                    .collect()
            }
        })
        .collect()
}

#[cfg(any(feature = "desktop", feature = "esp32"))]
fn format_brave_results(body: &serde_json::Value, query: &str, count: usize) -> ToolResult {
    let results = body
        .pointer("/web/results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let formatted: Vec<String> = results
        .iter()
        .take(count)
        .enumerate()
        .map(|(i, r)| {
            let title = r["title"].as_str().unwrap_or("(no title)");
            let url = r["url"].as_str().unwrap_or("");
            let desc = r["description"].as_str().unwrap_or("");
            format!("{}. [{}]({})\n   {}", i + 1, title, url, desc)
        })
        .collect();

    if formatted.is_empty() {
        ToolResult::Text(format!("No results found for '{}'", query))
    } else {
        ToolResult::Text(formatted.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(feature = "desktop")]
    fn desktop_refusal_names_size_and_cap() {
        let msg = super::refusal_message_desktop(50_000, 8000);
        assert!(msg.contains("50000 bytes"));
        assert!(msg.contains("8000-byte fetch cap"));
        assert!(msg.contains("larger max_length"));
        // No original bytes leaked.
        assert!(!msg.contains("xxx"));
    }

    #[test]
    #[cfg(any(feature = "desktop", feature = "esp32"))]
    fn method_parse_is_case_insensitive_and_complete() {
        use super::HttpMethod::*;
        for (s, want) in [
            ("get", Get),
            ("GET", Get),
            (" post ", Post),
            ("Put", Put),
            ("patch", Patch),
            ("DELETE", Delete),
            ("head", Head),
        ] {
            assert_eq!(super::HttpMethod::parse(s).unwrap(), want, "parsing {:?}", s);
        }
    }

    #[test]
    #[cfg(any(feature = "desktop", feature = "esp32"))]
    fn method_parse_rejects_unknown_with_actionable_error() {
        let err = super::HttpMethod::parse("TRACE").unwrap_err();
        assert!(err.contains("TRACE"));
        assert!(err.contains("GET, POST, PUT, PATCH, DELETE"));
    }

    #[test]
    #[cfg(any(feature = "desktop", feature = "esp32"))]
    fn only_write_verbs_carry_a_body() {
        use super::HttpMethod::*;
        assert!(Post.allows_body());
        assert!(Put.allows_body());
        assert!(Patch.allows_body());
        assert!(Delete.allows_body());
        assert!(!Get.allows_body());
        assert!(!Head.allows_body());
    }

    #[test]
    #[cfg(any(feature = "desktop", feature = "esp32"))]
    fn parse_headers_reads_string_map_and_defaults_empty() {
        let none = serde_json::json!({});
        assert!(super::parse_headers(&none).unwrap().is_empty());

        let args = serde_json::json!({
            "headers": { "Authorization": "Bearer abc", "Accept": "application/json" }
        });
        let mut h = super::parse_headers(&args).unwrap();
        h.sort();
        assert_eq!(
            h,
            vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("Authorization".to_string(), "Bearer abc".to_string()),
            ]
        );
    }

    #[test]
    #[cfg(any(feature = "desktop", feature = "esp32"))]
    fn parse_headers_rejects_non_string_values_and_non_objects() {
        let bad_val = serde_json::json!({ "headers": { "X-Count": 5 } });
        assert!(super::parse_headers(&bad_val).is_err());

        let bad_shape = serde_json::json!({ "headers": "Authorization: Bearer x" });
        assert!(super::parse_headers(&bad_shape).is_err());
    }
}
