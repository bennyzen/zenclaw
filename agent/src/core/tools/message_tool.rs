use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct MessageTool;

#[async_trait]
impl Tool for MessageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "message_send".to_string(),
            description: "Send a message to a channel or chat. Actions: send to cli (stdout) or telegram.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "enum": ["cli", "telegram"],
                        "description": "Target channel"
                    },
                    "chat_id": {
                        "type": "string",
                        "description": "Target chat ID"
                    },
                    "text": {
                        "type": "string",
                        "description": "Message text"
                    }
                },
                "required": ["channel", "chat_id", "text"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let channel = args.get("channel").and_then(|v| v.as_str()).unwrap_or("cli");
        let chat_id = args.get("chat_id").and_then(|v| v.as_str()).unwrap_or("");
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");

        match channel {
            "cli" => {
                println!("[message_send -> {}] {}", chat_id, text);
                ToolResult::Text("Delivered to cli.".to_string())
            }
            "telegram" => {
                #[cfg(feature = "desktop")]
                {
                    let tg_config = match &ctx.config.channels.telegram {
                        Some(cfg) if cfg.enabled && !cfg.bot_token.is_empty() => cfg,
                        _ => {
                            return ToolResult::Error(
                                "Telegram not configured or not enabled.".to_string(),
                            );
                        }
                    };

                    let client = reqwest::Client::new();
                    let url = format!(
                        "https://api.telegram.org/bot{}/sendMessage",
                        tg_config.bot_token
                    );
                    let body = json!({
                        "chat_id": chat_id,
                        "text": text,
                    });

                    tracing::info!(chat_id, text_len = text.len(), "message_send -> telegram");

                    match client.post(&url).json(&body).send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            if status.is_success() {
                                ToolResult::Text("Delivered to telegram.".to_string())
                            } else {
                                let err_body = resp.text().await.unwrap_or_default();
                                ToolResult::Error(format!(
                                    "Telegram API error {}: {}",
                                    status, err_body
                                ))
                            }
                        }
                        Err(e) => ToolResult::Error(format!("Telegram request failed: {}", e)),
                    }
                }

                #[cfg(not(feature = "desktop"))]
                {
                    let _ = ctx;
                    ToolResult::Error("Telegram not available on this platform.".to_string())
                }
            }
            other => ToolResult::Error(format!("Unknown channel: '{}'", other)),
        }
    }
}
