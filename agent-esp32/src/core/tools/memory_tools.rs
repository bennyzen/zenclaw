use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct MemoryTool;

#[async_trait]
impl Tool for MemoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory".to_string(),
            description: "Persistent memory. Actions: save, search, get, reindex.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["save", "search", "get", "reindex"],
                        "description": "Operation to perform"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to save (save) or query string (search)"
                    },
                    "tags": {
                        "type": "string",
                        "description": "Comma-separated tags (save)"
                    },
                    "id": {
                        "type": "string",
                        "description": "Memory entry ID (get)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");

        match action {
            "save" => do_save(&args, ctx),
            "search" => do_search(&args, ctx),
            "get" => do_get(&args, ctx),
            "reindex" => do_reindex(ctx),
            _ => ToolResult::Error(format!("Unknown action '{}'", action)),
        }
    }
}

fn memory_path(ctx: &ToolContext) -> String {
    format!("{}/MEMORY.md", ctx.data_dir)
}

fn do_save(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let content = match args["content"].as_str() {
        Some(c) => c,
        None => return ToolResult::Error("Missing 'content' for save".to_string()),
    };
    let tags = args["tags"].as_str().unwrap_or("");

    let path = memory_path(ctx);

    // Ensure directory exists
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return ToolResult::Error(format!("Failed to create memory dir: {}", e));
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let entry = if tags.is_empty() {
        format!("\n## [{}] {}\n{}\n", id, timestamp, content)
    } else {
        format!("\n## [{}] {} (tags: {})\n{}\n", id, timestamp, tags, content)
    };

    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);

    match file {
        Ok(mut f) => match f.write_all(entry.as_bytes()) {
            Ok(()) => ToolResult::Text(format!("Saved memory {}", id)),
            Err(e) => ToolResult::Error(format!("Write failed: {}", e)),
        },
        Err(e) => ToolResult::Error(format!("Failed to open memory file: {}", e)),
    }
}

fn do_search(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let query = match args["content"].as_str() {
        Some(q) => q.to_lowercase(),
        None => return ToolResult::Error("Missing 'content' (query) for search".to_string()),
    };

    let path = memory_path(ctx);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ToolResult::Text("No memories found.".to_string()),
    };

    let keywords: Vec<&str> = query.split_whitespace().collect();
    let mut results: Vec<String> = Vec::new();

    // Split into sections by ## headers
    let sections: Vec<&str> = content.split("\n## ").collect();
    for section in sections.iter().skip(1) {
        let lower = section.to_lowercase();
        if keywords.iter().any(|kw| lower.contains(kw)) {
            results.push(format!("## {}", section.trim()));
        }
    }

    if results.is_empty() {
        ToolResult::Text("No matching memories found.".to_string())
    } else {
        ToolResult::Text(results.join("\n\n"))
    }
}

fn do_get(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
    let id = match args["id"].as_str() {
        Some(i) => i,
        None => return ToolResult::Error("Missing 'id' for get".to_string()),
    };

    let path = memory_path(ctx);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ToolResult::Error("No memory file found.".to_string()),
    };

    let sections: Vec<&str> = content.split("\n## ").collect();
    for section in sections.iter().skip(1) {
        if section.contains(id) {
            return ToolResult::Text(format!("## {}", section.trim()));
        }
    }

    ToolResult::Error(format!("Memory '{}' not found.", id))
}

fn do_reindex(ctx: &ToolContext) -> ToolResult {
    let path = memory_path(ctx);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ToolResult::Text("Reindex complete. 0 entries.".to_string()),
    };

    let count = content.matches("\n## ").count();
    ToolResult::Text(format!("Reindex complete. {} entries.", count))
}
