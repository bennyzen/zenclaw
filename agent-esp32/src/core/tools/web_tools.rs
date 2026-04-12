use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

#[cfg(feature = "esp32")]
fn esp_http_get(url: &str, max_length: usize) -> Result<(u16, String), String> {
    esp_http_get_with_headers(url, &[], max_length)
}

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
    let mut body = Vec::new();
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
        if body.len() >= max_length { break; }
    }
    let text = String::from_utf8_lossy(&body);
    let truncated = if text.len() > max_length {
        format!("{}\n\n[truncated at {} of {}+ chars]", &text[..max_length], max_length, text.len())
    } else {
        text.into_owned()
    };
    Ok((status, truncated))
}

/// Fetches content from a URL.
pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch content from a URL.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch"
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Max response length in chars (default 8000)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let url = match args["url"].as_str() {
            Some(u) => u,
            None => return ToolResult::Error("Missing 'url'".to_string()),
        };
        let max_length = args["max_length"].as_u64().unwrap_or(8000) as usize;

        #[cfg(feature = "desktop")]
        {
            match reqwest::get(url).await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    match resp.text().await {
                        Ok(body) => {
                            let truncated = if body.len() > max_length {
                                format!(
                                    "{}\n\n[truncated at {} of {} chars]",
                                    &body[..max_length],
                                    max_length,
                                    body.len()
                                )
                            } else {
                                body
                            };
                            ToolResult::Text(format!("HTTP {} — {}", status, truncated))
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
            ToolResult::Error("web_fetch not available on this platform".to_string())
        }
    }
}

/// Web search using Brave Search API.
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "count": {
                        "type": "integer",
                        "description": "Number of results (default 5, max 20)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let query = match args["query"].as_str() {
            Some(q) if !q.is_empty() => q,
            _ => return ToolResult::Error("Missing 'query'".to_string()),
        };
        let count = args["count"].as_u64().unwrap_or(5).min(20) as usize;

        let api_key = ctx
            .config
            .providers
            .entries
            .get("brave")
            .and_then(|e| e.api_key.clone());

        #[cfg(feature = "desktop")]
        {
            let api_key = match api_key {
                Some(k) if !k.is_empty() => k,
                _ => {
                    return ToolResult::Text(
                        "Web search not configured. Add a 'brave' provider with your Brave Search API key in config. \
                         You can still use web_fetch to read URLs, or rely on Gemini's built-in Google Search grounding."
                            .to_string(),
                    )
                }
            };

            let encoded: String = query.chars().map(|c| match c {
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
            }).collect();

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

        #[cfg(feature = "esp32")]
        {
            let api_key = match api_key {
                Some(k) if !k.is_empty() => k,
                _ => {
                    return ToolResult::Text(
                        "Web search not configured. Add a 'brave' provider with your Brave Search API key in config."
                            .to_string(),
                    )
                }
            };

            let encoded: String = query.chars().map(|c| match c {
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
            }).collect();

            let url = format!(
                "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
                encoded, count
            );

            match esp_http_get_with_headers(&url, &[
                ("Accept", "application/json"),
                ("X-Subscription-Token", &api_key),
            ], 16000) {
                Ok((_status, body)) => {
                    let data: serde_json::Value = match serde_json::from_str(&body) {
                        Ok(j) => j,
                        Err(e) => return ToolResult::Error(format!("Parse error: {}", e)),
                    };
                    let results = data.pointer("/web/results")
                        .and_then(|r| r.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let formatted: Vec<String> = results.iter().take(count).enumerate()
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
                Err(e) => ToolResult::Error(format!("Search failed: {}", e)),
            }
        }

        #[cfg(not(any(feature = "desktop", feature = "esp32")))]
        {
            let _ = (query, count, api_key);
            ToolResult::Error("web_search not available on this platform".to_string())
        }
    }
}
