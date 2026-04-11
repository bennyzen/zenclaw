use crate::core::types::ToolDefinition;

/// Build the system prompt from SOUL.md, tools, skills, and runtime info.
pub fn build_system_prompt(
    _soul_md: &str,
    _agent_name: &str,
    _tools: &[ToolDefinition],
) -> String {
    // TODO: implement prompt assembly
    todo!("prompt::build_system_prompt")
}
