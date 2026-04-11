use crate::core::types::{LlmResponse, Message, ToolDefinition};

/// Provider dispatch with retry. Selects model, handles streaming.
pub struct Runner {
    // TODO: hold provider config, http client
}

impl Runner {
    pub fn new() -> Self {
        Self {}
    }

    /// Send messages to the LLM and get a response.
    pub async fn call(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: implement provider dispatch
        todo!("Runner::call")
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}
