use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

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

        #[cfg(not(feature = "desktop"))]
        {
            let _ = (url, max_length);
            ToolResult::Error("web_fetch requires the desktop feature".to_string())
        }
    }
}

/// Web search stub.
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web (requires search provider config).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::Error("Web search not configured. Set search provider in config.".to_string())
    }
}
