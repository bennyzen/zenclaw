use crate::core::types::{LlmResponse, Message};
use crate::core::tools::ToolRegistry;

/// Run the LLM <-> tool execution loop until a text response is produced.
pub async fn run_loop(
    _messages: &mut Vec<Message>,
    _tools: &ToolRegistry,
    _chat_id: &str,
    _max_iterations: usize,
) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
    // TODO: implement LLM call -> tool execution -> repeat
    todo!("agent_loop::run_loop")
}
