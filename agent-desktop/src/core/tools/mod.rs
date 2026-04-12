pub mod cron_tools;
pub mod file_tools;
pub mod gateway_tool;
pub mod gsheets_tools;
pub mod mcp_tools;
pub mod memory_tools;
pub mod message_tool;
pub mod session_tools;
pub mod storage_tools;
pub mod subagent_tools;
pub mod web_tools;

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use crate::core::sessions::SessionManager;
use crate::core::types::ToolDefinition;

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub enum ToolResult {
    Text(String),
    Error(String),
    Json(serde_json::Value),
}

impl std::fmt::Display for ToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolResult::Text(s) => write!(f, "{}", s),
            ToolResult::Error(s) => write!(f, "Error: {}", s),
            ToolResult::Json(v) => write!(f, "{}", v),
        }
    }
}

/// Context passed to tool execution — carries shared gateway state.
pub struct ToolContext {
    pub chat_id: String,
    pub prompt_source: Option<String>,
    pub config: Arc<Config>,
    pub sessions: Arc<SessionManager>,
    pub data_dir: String,
}

/// Trait for an executable tool.
#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult;
}

/// Registry holding all registered tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let def = tool.definition();
        self.tools.insert(def.name.clone(), tool);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Returns tool definitions in OpenAI function-calling format.
    pub fn get_tools_for_llm(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| {
                let def = t.definition();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": def.name,
                        "description": def.description,
                        "parameters": def.parameters,
                    }
                })
            })
            .collect()
    }

    /// Validate args against a tool's parameter schema, then execute.
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        let tool = match self.tools.get(name) {
            Some(t) => t,
            None => return ToolResult::Error(format!("Unknown tool: {}", name)),
        };

        let def = tool.definition();
        if let Some(err) = validate_args(&args, &def.parameters) {
            return ToolResult::Error(err);
        }

        tool.execute(args, ctx).await
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate tool arguments against a JSON Schema parameters object.
/// Returns `Some(error_message)` on validation failure, `None` if valid.
fn validate_args(args: &serde_json::Value, schema: &serde_json::Value) -> Option<String> {
    let args_obj = match args.as_object() {
        Some(o) => o,
        None => return Some("Arguments must be a JSON object".to_string()),
    };

    // Check required fields
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for field in required {
            if let Some(name) = field.as_str() {
                if !args_obj.contains_key(name) {
                    return Some(format!("Missing required parameter: '{}'", name));
                }
            }
        }
    }

    // Type-check provided fields against properties
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return None,
    };

    for (field, value) in args_obj {
        if field.starts_with('_') {
            continue; // skip injected context fields
        }
        let spec = match properties.get(field) {
            Some(s) => s,
            None => continue,
        };

        // Type check
        if let Some(expected_type) = spec.get("type").and_then(|t| t.as_str()) {
            let type_ok = match expected_type {
                "string" => value.is_string(),
                "number" => value.is_number(),
                "integer" => value.is_i64() || value.is_u64(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                _ => true,
            };
            if !type_ok {
                return Some(format!(
                    "Parameter '{}' must be {}, got {}",
                    field,
                    expected_type,
                    json_type_name(value),
                ));
            }
        }

        // Enum check
        if let Some(enum_values) = spec.get("enum").and_then(|e| e.as_array()) {
            if !enum_values.contains(value) {
                return Some(format!(
                    "Parameter '{}' must be one of {:?}, got '{}'",
                    field,
                    enum_values
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>(),
                    value,
                ));
            }
        }
    }

    None
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_args_passes_valid() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["read", "write"]},
                "path": {"type": "string"},
            },
            "required": ["action"],
        });
        let args = serde_json::json!({"action": "read", "path": "/tmp/foo"});
        assert!(validate_args(&args, &schema).is_none());
    }

    #[test]
    fn validate_args_missing_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string"},
            },
            "required": ["action"],
        });
        let args = serde_json::json!({});
        let err = validate_args(&args, &schema).unwrap();
        assert!(err.contains("Missing required parameter: 'action'"));
    }

    #[test]
    fn validate_args_wrong_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer"},
            },
            "required": [],
        });
        let args = serde_json::json!({"count": "not a number"});
        let err = validate_args(&args, &schema).unwrap();
        assert!(err.contains("must be integer"));
    }

    #[test]
    fn validate_args_bad_enum() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["read", "write"]},
            },
            "required": ["action"],
        });
        let args = serde_json::json!({"action": "delete"});
        let err = validate_args(&args, &schema).unwrap();
        assert!(err.contains("must be one of"));
    }

    #[test]
    fn validate_args_skips_underscore_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string"},
            },
            "required": ["action"],
        });
        // _chat_id is injected by the executor — should not be validated
        let args = serde_json::json!({"action": "read", "_chat_id": 123});
        assert!(validate_args(&args, &schema).is_none());
    }

    #[test]
    fn get_tools_for_llm_format() {
        let mut registry = ToolRegistry::new();

        struct DummyTool;
        #[async_trait]
        impl Tool for DummyTool {
            fn definition(&self) -> ToolDefinition {
                ToolDefinition {
                    name: "test".to_string(),
                    description: "A test tool".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                    }),
                }
            }
            async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
                ToolResult::Text("ok".to_string())
            }
        }

        registry.register(Box::new(DummyTool));
        let llm_tools = registry.get_tools_for_llm();
        assert_eq!(llm_tools.len(), 1);
        assert_eq!(llm_tools[0]["type"], "function");
        assert_eq!(llm_tools[0]["function"]["name"], "test");
    }
}
