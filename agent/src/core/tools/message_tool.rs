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
            description: "Send a message to a channel or chat.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Target channel (e.g. 'telegram', 'cli')"
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
                "required": ["text"]
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::Text("Message delivery not yet wired.".to_string())
    }
}
