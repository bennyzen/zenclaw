use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

#[cfg(feature = "esp32")]
fn esp_http_get(url: &str, max_length: usize) -> Result<(u16, String), String> {
    esp_http_get_with_headers(url, &[], max_length)
}

/// Returns the response body if it fits in `max_length` bytes, otherwise
/// returns an actionable refusal as the body (with the original HTTP status
/// preserved). Never truncates: the caller and the LLM both prefer "too
/// big, narrow your scope" over "here's an arbitrary prefix".
#[cfg(feature = "esp32")]
fn esp_http_get_with_headers(url: &str, headers: &[(&str, &str)], max_length: usize) -> Result<(u16, String), String> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    use esp_idf_svc::http::Method;
    use std::io::Read;

    let config = HttpConfig {
        buffer_size: Some(4096),
        buffer_size_tx: Some(1024),
        timeout: Some(std::time::Duration::from_secs(15)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut conn = EspHttpConnection::new(&config).map_err(|e| format!("HTTP: {}", e))?;
    conn.initiate_request(Method::Get, url, headers).map_err(|e| format!("req: {}", e))?;
    conn.initiate_response().map_err(|e| format!("resp: {}", e))?;
    let status = conn.status();

    let mut buf = [0u8; 4096];
    let mut body: Vec<u8> = Vec::with_capacity(8192);
    let mut overflowed = false;
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 { break; }
        // Memory bound: stop reading once max_length would be exceeded so
        // we don't fill heap with bytes we're going to refuse anyway.
        if body.len() + n > max_length {
            overflowed = true;
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }

    if overflowed {
        return Ok((status, refusal_message(max_length)));
    }
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

#[cfg(feature = "esp32")]
fn refusal_message(cap: usize) -> String {
    format!(
        "Response body exceeded the {}-byte fetch cap. Re-run with a larger \
         max_length, target a more specific URL, or fetch a sub-range.",
        cap,
    )
}

pub struct WebTool;

#[async_trait]
impl Tool for WebTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web".to_string(),
            description: "Web access. Actions:\n\
                - fetch: get content from a URL. Oversized bodies are refused with an actionable error rather than truncated.\n\
                - search: web search via Brave Search API. Requires search.brave_api_key in config.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["fetch", "search"],
                        "description": "Operation to perform"
                    },
                    "url":        { "type": "string",  "description": "URL to fetch (action=fetch)." },
                    "max_length": { "type": "integer", "description": "Maximum response body bytes (fetch). Oversized bodies are refused, not truncated. Default suits typical pages on the current platform." },
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
            "fetch"  => do_fetch(&args).await,
            "search" => do_search(&args, ctx).await,
            "" => ToolResult::Error("web: 'action' is required".into()),
            other => ToolResult::Error(format!("web: unknown action '{}'", other)),
        }
    }
}

// --- per-action implementations ---

async fn do_fetch(args: &serde_json::Value) -> ToolResult {
    let url = match args["url"].as_str() {
        Some(u) => u,
        None => return ToolResult::Error("web(fetch): 'url' is required".into()),
    };
    // Defaults: desktop is generous (real pages flow through unchanged),
    // ESP32 caps at 32KB which fits comfortably in PSRAM heap and covers
    // most useful HTML/text resources.
    #[cfg(feature = "desktop")]
    let default_max = 10 * 1024 * 1024;
    #[cfg(feature = "esp32")]
    let default_max = 32 * 1024;
    #[cfg(not(any(feature = "desktop", feature = "esp32")))]
    let default_max = 8 * 1024;
    let max_length = args["max_length"].as_u64().unwrap_or(default_max as u64) as usize;

    #[cfg(feature = "desktop")]
    {
        match reqwest::get(url).await {
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
            Err(e) => ToolResult::Error(format!("Fetch failed: {}", e)),
        }
    }

    #[cfg(feature = "esp32")]
    {
        match esp_http_get(url, max_length) {
            Ok((status, body)) => ToolResult::Text(format!("HTTP {} — {}", status, body)),
            Err(e) => ToolResult::Error(format!("Fetch failed: {}", e)),
        }
    }

    #[cfg(not(any(feature = "desktop", feature = "esp32")))]
    {
        let _ = (url, max_length);
        ToolResult::Error("web(fetch) not available on this platform".into())
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
        match esp_http_get_with_headers(&url, &[
            ("Accept", "application/json"),
            ("X-Subscription-Token", &api_key),
        ], 64 * 1024) {
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
}
