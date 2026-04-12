use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct GSheetsTool;

#[async_trait]
impl Tool for GSheetsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "gsheets".to_string(),
            description: "Google Sheets integration. Actions: read, write, append, list.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "write", "append", "list"],
                        "description": "Operation to perform"
                    },
                    "spreadsheet_id": {
                        "type": "string",
                        "description": "Google Sheets spreadsheet ID"
                    },
                    "range": {
                        "type": "string",
                        "description": "Cell range (e.g. 'Sheet1!A1:C10')"
                    },
                    "values": {
                        "type": "array",
                        "description": "Row data to write/append"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        ToolResult::Text(format!(
            "gsheets '{}' not yet implemented (requires google.client_id in config)",
            action
        ))
    }
}
