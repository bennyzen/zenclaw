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

/// Per-entry content cap when serving session history. Without this,
/// a single 491 KB web_fetch tool result inside the recent window would
/// be returned verbatim, feeding the model its own bloat (the 612 KB
/// self-DoS observed on the baseline 50-turn synthetic test, turn 42).
/// Tuned to fit typical chat messages and short tool results while
/// clipping outlier payloads.
const HISTORY_PER_ENTRY_CAP: usize = 4096;

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

    let mut out: Vec<String> = Vec::with_capacity(limit);
    for line in &lines[start..] {
        out.push(clip_history_line(line));
    }
    ToolResult::Text(out.join("\n"))
}

/// If the line is a JSON entry whose `content` field is longer than the
/// per-entry cap, replace `content` with a short marker. Non-JSON or
/// small lines pass through unchanged.
fn clip_history_line(line: &str) -> String {
    if line.len() <= HISTORY_PER_ENTRY_CAP {
        return line.to_string();
    }
    let mut value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return line.to_string(),
    };
    if let Some(obj) = value.as_object_mut() {
        if let Some(content) = obj.get_mut("content") {
            if let Some(s) = content.as_str() {
                if s.len() > HISTORY_PER_ENTRY_CAP {
                    *content = json!(format!(
                        "[content omitted: {} bytes — exceeds session.history per-entry cap]",
                        s.len()
                    ));
                }
            }
        }
    }
    serde_json::to_string(&value).unwrap_or_else(|_| line.to_string())
}
