use async_trait::async_trait;
use serde_json::json;
use std::fs;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct StorageTool;

#[async_trait]
impl Tool for StorageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "storage".to_string(),
            description: "Cloud storage (S3). Actions: read, write, delete, list, info, grep, analyze.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "write", "delete", "list", "info", "grep", "analyze"],
                        "description": "Operation to perform"
                    },
                    "key": {
                        "type": "string",
                        "description": "Storage key / file path"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (grep)"
                    },
                    "prefix": {
                        "type": "string",
                        "description": "List prefix filter"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        let storage_dir = format!("{}/storage", ctx.data_dir);

        match action {
            "read" => {
                let key = match args["key"].as_str() {
                    Some(k) => k,
                    None => return ToolResult::Error("Missing 'key'".to_string()),
                };
                if key.contains("..") {
                    return ToolResult::Error("Invalid key".to_string());
                }
                let path = format!("{}/{}", storage_dir, key);
                match fs::read_to_string(&path) {
                    Ok(content) => ToolResult::Text(content),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        ToolResult::Error(format!("Key '{}' not found", key))
                    }
                    Err(e) => ToolResult::Error(format!("Read error: {}", e)),
                }
            }
            "write" => {
                let key = match args["key"].as_str() {
                    Some(k) => k,
                    None => return ToolResult::Error("Missing 'key'".to_string()),
                };
                let content = match args["content"].as_str() {
                    Some(c) => c,
                    None => return ToolResult::Error("Missing 'content'".to_string()),
                };
                if key.contains("..") {
                    return ToolResult::Error("Invalid key".to_string());
                }
                let path = format!("{}/{}", storage_dir, key);
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = fs::create_dir_all(parent);
                }
                match fs::write(&path, content) {
                    Ok(_) => ToolResult::Text(format!("Wrote {} bytes to '{}'", content.len(), key)),
                    Err(e) => ToolResult::Error(format!("Write error: {}", e)),
                }
            }
            "delete" => {
                let key = match args["key"].as_str() {
                    Some(k) => k,
                    None => return ToolResult::Error("Missing 'key'".to_string()),
                };
                let path = format!("{}/{}", storage_dir, key);
                match fs::remove_file(&path) {
                    Ok(_) => ToolResult::Text(format!("Deleted '{}'", key)),
                    Err(e) => ToolResult::Error(format!("Delete error: {}", e)),
                }
            }
            "list" => {
                let prefix = args["prefix"].as_str().unwrap_or("");
                let dir = if prefix.is_empty() {
                    storage_dir.clone()
                } else {
                    format!("{}/{}", storage_dir, prefix)
                };
                match fs::read_dir(&dir) {
                    Ok(entries) => {
                        let keys: Vec<String> = entries
                            .filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().to_string())
                            .collect();
                        ToolResult::Json(json!({"keys": keys, "count": keys.len()}))
                    }
                    Err(_) => ToolResult::Json(json!({"keys": [], "count": 0})),
                }
            }
            "info" => {
                let exists = std::path::Path::new(&storage_dir).exists();
                let count = if exists {
                    fs::read_dir(&storage_dir)
                        .map(|e| e.filter_map(|e| e.ok()).count())
                        .unwrap_or(0)
                } else {
                    0
                };
                ToolResult::Json(json!({"type": "local", "path": storage_dir, "exists": exists, "file_count": count}))
            }
            "grep" => {
                let pattern = match args["pattern"].as_str() {
                    Some(p) => p,
                    None => return ToolResult::Error("Missing 'pattern'".to_string()),
                };
                let pattern_lower = pattern.to_lowercase();
                let mut matches = Vec::new();
                if let Ok(entries) = fs::read_dir(&storage_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            for (i, line) in content.lines().enumerate() {
                                if line.to_lowercase().contains(&pattern_lower) {
                                    matches.push(json!({"file": name, "line": i + 1, "text": &line[..line.len().min(200)]}));
                                    if matches.len() >= 50 { break; }
                                }
                            }
                        }
                        if matches.len() >= 50 { break; }
                    }
                }
                ToolResult::Json(json!({"matches": matches, "count": matches.len()}))
            }
            "analyze" => ToolResult::Text("Storage analyze not yet implemented.".to_string()),
            _ => ToolResult::Error(format!("Unknown action '{}'", action)),
        }
    }
}
