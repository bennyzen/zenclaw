use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct McpTool;

#[async_trait]
impl Tool for McpTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "mcp".to_string(),
            description: "Model Context Protocol. Actions: connect, list_tools, call, disconnect, servers.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["connect", "list_tools", "call", "disconnect", "servers"],
                        "description": "Operation to perform"
                    },
                    "server": {
                        "type": "string",
                        "description": "MCP server name or URL"
                    },
                    "tool_name": {
                        "type": "string",
                        "description": "Tool to call (call action)"
                    },
                    "tool_args": {
                        "type": "object",
                        "description": "Arguments for the tool call"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        ToolResult::Text(format!("mcp '{}' not yet implemented", action))
    }
}
