use async_trait::async_trait;
use serde_json::json;

use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct FileTool;

#[async_trait]
impl Tool for FileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "file".to_string(),
            description: "File operations. Actions: read, write, edit, delete, list_dir.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "write", "edit", "delete", "list_dir"],
                        "description": "Operation to perform"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory path (relative to data dir)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write (write action)"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "String to find (edit action)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement string (edit action)"
                    }
                },
                "required": ["action", "path"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        let path = args["path"].as_str().unwrap_or("");

        if path.contains("..") {
            return ToolResult::Error("Path traversal not allowed".to_string());
        }

        let full_path = format!("{}/{}", ctx.data_dir, path);

        match action {
            "read" => do_read(&full_path),
            "write" => {
                let content = match args["content"].as_str() {
                    Some(c) => c,
                    None => return ToolResult::Error("Missing 'content' for write".to_string()),
                };
                do_write(&full_path, content)
            }
            "edit" => {
                let old_string = match args["old_string"].as_str() {
                    Some(s) => s,
                    None => return ToolResult::Error("Missing 'old_string' for edit".to_string()),
                };
                let new_string = match args["new_string"].as_str() {
                    Some(s) => s,
                    None => return ToolResult::Error("Missing 'new_string' for edit".to_string()),
                };
                do_edit(&full_path, old_string, new_string)
            }
            "delete" => do_delete(&full_path),
            "list_dir" => do_list_dir(&full_path),
            _ => ToolResult::Error(format!("Unknown action '{}'", action)),
        }
    }
}

fn do_read(path: &str) -> ToolResult {
    match std::fs::read_to_string(path) {
        Ok(content) => ToolResult::Text(content),
        Err(e) => ToolResult::Error(format!("Failed to read '{}': {}", path, e)),
    }
}

fn do_write(path: &str, content: &str) -> ToolResult {
    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(path).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return ToolResult::Error(format!("Failed to create directory: {}", e));
        }
    }
    match std::fs::write(path, content) {
        Ok(()) => ToolResult::Text(format!("Wrote {} bytes to {}", content.len(), path)),
        Err(e) => ToolResult::Error(format!("Failed to write '{}': {}", path, e)),
    }
}

fn do_edit(path: &str, old_string: &str, new_string: &str) -> ToolResult {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return ToolResult::Error(format!("Failed to read '{}': {}", path, e)),
    };

    let count = content.matches(old_string).count();
    if count == 0 {
        return ToolResult::Error("old_string not found in file".to_string());
    }

    let new_content = content.replacen(old_string, new_string, 1);
    match std::fs::write(path, &new_content) {
        Ok(()) => ToolResult::Text(format!(
            "Replaced 1 occurrence ({} total matches) in {}",
            count, path
        )),
        Err(e) => ToolResult::Error(format!("Failed to write '{}': {}", path, e)),
    }
}

fn do_delete(path: &str) -> ToolResult {
    match std::fs::remove_file(path) {
        Ok(()) => ToolResult::Text(format!("Deleted {}", path)),
        Err(e) => ToolResult::Error(format!("Failed to delete '{}': {}", path, e)),
    }
}

fn do_list_dir(path: &str) -> ToolResult {
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => return ToolResult::Error(format!("Failed to list '{}': {}", path, e)),
    };

    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => {
                let name = e.file_name().to_string_lossy().to_string();
                let suffix = if e.path().is_dir() { "/" } else { "" };
                names.push(format!("{}{}", name, suffix));
            }
            Err(e) => {
                names.push(format!("(error: {})", e));
            }
        }
    }
    names.sort();
    ToolResult::Text(names.join("\n"))
}
