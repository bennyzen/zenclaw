use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct SessionTool;

#[async_trait]
impl Tool for SessionTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "session".to_string(),
            description: "Session management. Actions: status, list, history.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "list", "history"],
                        "description": "Operation to perform"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max entries to return (history, default 20)"
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
            "list" => do_list(ctx),
            "history" => {
                let limit = args["limit"].as_u64().unwrap_or(20) as usize;
                do_history(ctx, limit)
            }
            _ => ToolResult::Error(format!("Unknown action '{}'", action)),
        }
    }
}

fn do_status(ctx: &ToolContext) -> ToolResult {
    let platform = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let info = json!({
        "chat_id": ctx.chat_id,
        "platform": platform,
        "data_dir": ctx.data_dir,
        "agent_name": ctx.config.agent_name,
    });
    ToolResult::Json(info)
}

fn do_list(ctx: &ToolContext) -> ToolResult {
    let sessions_dir = format!("{}/sessions", ctx.data_dir);
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return ToolResult::Text("No sessions found.".to_string()),
    };

    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".jsonl") {
            names.push(name);
        }
    }
    names.sort();

    if names.is_empty() {
        ToolResult::Text("No sessions found.".to_string())
    } else {
        ToolResult::Text(names.join("\n"))
    }
}

fn do_history(ctx: &ToolContext, limit: usize) -> ToolResult {
    let path = format!("{}/sessions/{}.jsonl", ctx.data_dir, ctx.chat_id);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ToolResult::Text("No history for this session.".to_string()),
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = if lines.len() > limit {
        lines.len() - limit
    } else {
        0
    };

    let recent: Vec<&str> = lines[start..].to_vec();
    ToolResult::Text(recent.join("\n"))
}
