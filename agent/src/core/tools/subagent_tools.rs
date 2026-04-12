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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        ToolResult::Text(format!("subagent '{}' not yet implemented", action))
    }
}
