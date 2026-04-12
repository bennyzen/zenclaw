use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct GatewayTool;

#[async_trait]
impl Tool for GatewayTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "gateway".to_string(),
            description: "Gateway management. Actions: status, reload.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "reload"],
                        "description": "Operation to perform"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");

        match action {
            "status" => do_status(ctx),
            "reload" => do_reload(),
            _ => ToolResult::Error(format!("Unknown action '{}'", action)),
        }
    }
}

fn do_status(ctx: &ToolContext) -> ToolResult {
    let info = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "agent_name": ctx.config.agent_name,
        "data_dir": ctx.data_dir,
        "runtime": "rust",
    });
    ToolResult::Json(info)
}

fn do_reload() -> ToolResult {
    // Stub: config reload not yet implemented
    ToolResult::Text("Config reload not yet implemented.".to_string())
}
