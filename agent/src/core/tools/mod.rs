use async_trait::async_trait;
use std::collections::HashMap;

use crate::core::types::ToolDefinition;

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub enum ToolResult {
    Text(String),
    Error(String),
    Json(serde_json::Value),
}

/// Context passed to tool execution.
pub struct ToolContext {
    pub chat_id: String,
    pub prompt_source: Option<String>,
}

/// Trait for an executable tool.
#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult;
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

    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        match self.tools.get(name) {
            Some(tool) => tool.execute(args, ctx).await,
            None => ToolResult::Error(format!("Unknown tool: {}", name)),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
