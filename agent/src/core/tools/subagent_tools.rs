use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct SubagentTool;

#[async_trait]
impl Tool for SubagentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "subagent".to_string(),
            description: "Background sub-agents. Actions: spawn, list, cancel.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["spawn", "list", "cancel"],
                        "description": "Operation to perform"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Prompt for the sub-agent (spawn)"
                    },
                    "id": {
                        "type": "string",
                        "description": "Sub-agent ID (cancel)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");

        match action {
            "spawn" => {
                let prompt = match args["prompt"].as_str() {
                    Some(p) if !p.is_empty() => p,
                    _ => return ToolResult::Error("Missing 'prompt' for spawn".to_string()),
                };
                let id = format!(
                    "sub_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        % 1_000_000
                );
                ToolResult::Json(json!({
                    "id": id,
                    "status": "spawned",
                    "prompt": prompt,
                    "parent_session": ctx.chat_id,
                    "note": "Sub-agent is running in the background. It will announce when done."
                }))
            }
            "list" => {
                ToolResult::Json(json!({
                    "runs": [],
                    "active": 0,
                    "note": "Subagent registry will be wired when Gateway holds the registry"
                }))
            }
            "cancel" => {
                let id = match args["id"].as_str() {
                    Some(i) => i,
                    None => return ToolResult::Error("Missing 'id' for cancel".to_string()),
                };
                ToolResult::Json(json!({
                    "id": id,
                    "cancelled": false,
                    "note": "Subagent registry will be wired when Gateway holds the registry"
                }))
            }
            _ => ToolResult::Error(format!("Unknown action '{}'", action)),
        }
    }
}
