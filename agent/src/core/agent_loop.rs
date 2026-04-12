use tokio_util::sync::CancellationToken;
use tracing::{info, error};

use crate::core::runner::{Runner, RunnerError};
use crate::core::tool_loop::{LoopDetector, LoopLevel};
use crate::core::tools::{ToolContext, ToolRegistry};
use crate::core::types::{LlmResponse, Message, MessageContent, ProviderData, Role, ToolCall};

const MAX_CONSECUTIVE_ERRORS: usize = 3;
const MAX_TOOL_RESULT_LEN: usize = 8000;

/// Run the LLM <-> tool execution loop until a text response is produced.
///
/// This is the core agent loop:
/// 1. Call LLM with messages + tools
/// 2. If response has tool_calls → execute them, append results, loop
/// 3. If response is text → return it
/// 4. Circuit breaker stops stuck loops
/// 5. Cancellation token allows aborting mid-turn
pub async fn run_loop(
    messages: &mut Vec<Message>,
    tools: &ToolRegistry,
    runner: &Runner,
    ctx: &ToolContext,
    cancel: Option<&CancellationToken>,
    model_override: Option<&str>,
) -> Result<String, AgentLoopError> {
    let tool_defs = tools.definitions();
    let mut consecutive_errors: usize = 0;
    let mut loop_detector = LoopDetector::new();
    let mut last_content = String::new();

    loop {
        // Check cancellation before LLM call
        if let Some(token) = cancel {
            if token.is_cancelled() {
                info!("Agent loop cancelled before LLM call");
                return Ok("Operation cancelled.".to_string());
            }
        }

        // Call LLM
        let response = match runner.call(messages, &tool_defs, model_override).await {
            Ok(r) => {
                consecutive_errors = 0;
                r
            }
            Err(e) => {
                consecutive_errors += 1;
                error!(attempt = consecutive_errors, error = %e, "LLM call failed");
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    return Err(AgentLoopError::TooManyErrors(e.to_string()));
                }
                continue;
            }
        };

        match response {
            LlmResponse::Text(text) => {
                last_content = text.clone();
                messages.push(Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(text),
                    tool_calls: None,
                    tool_call_id: None,
                    provider_data: None,
                });
                break;
            }
            LlmResponse::ToolCalls { tool_calls, provider_data } => {
                execute_tool_calls(
                    &tool_calls,
                    None,
                    provider_data,
                    messages,
                    tools,
                    ctx,
                    cancel,
                    &mut loop_detector,
                )
                .await?;
            }
            LlmResponse::Mixed { text, tool_calls, provider_data } => {
                last_content = text.clone();
                execute_tool_calls(
                    &tool_calls,
                    Some(&text),
                    provider_data,
                    messages,
                    tools,
                    ctx,
                    cancel,
                    &mut loop_detector,
                )
                .await?;
            }
        }
    }

    Ok(last_content)
}

async fn execute_tool_calls(
    tool_calls: &[ToolCall],
    assistant_text: Option<&str>,
    provider_data: Option<ProviderData>,
    messages: &mut Vec<Message>,
    tools: &ToolRegistry,
    ctx: &ToolContext,
    cancel: Option<&CancellationToken>,
    loop_detector: &mut LoopDetector,
) -> Result<(), AgentLoopError> {
    // provider_data carries thought_signatures from Gemini; attach to first tool call's message
    let mut remaining_provider_data = provider_data;

    for tc in tool_calls {
        let name = &tc.function.name;
        let args: serde_json::Value =
            serde_json::from_str(&tc.function.arguments).unwrap_or_default();

        // Check loop detector
        if let Some(check) = loop_detector.check(name, &args) {
            if check.level == LoopLevel::Critical {
                // Block execution — add error result
                push_tool_exchange(
                    messages,
                    tc,
                    assistant_text,
                    &check.message,
                    remaining_provider_data.take(),
                );
                info!(tool = %name, detector = check.detector, "Loop detector blocked tool call");
                continue;
            }
            // Warning — inject system message but still execute
            messages.push(Message {
                role: Role::System,
                content: MessageContent::Text(check.message),
                tool_calls: None,
                tool_call_id: None,
                provider_data: None,
            });
        }

        // Check cancellation
        if let Some(token) = cancel {
            if token.is_cancelled() {
                push_tool_exchange(messages, tc, assistant_text, "Skipped: operation cancelled.", remaining_provider_data.take());
                info!(tool = %name, "Tool skipped — cancelled");
                continue;
            }
        }

        loop_detector.record_call(name, &args);

        // Execute the tool
        let result = tools.execute(name, args.clone(), ctx).await;
        let result_str = result.to_string();

        loop_detector.record_outcome(name, &args, &result_str);

        // Trim large results
        let trimmed = soft_trim(&result_str, MAX_TOOL_RESULT_LEN);

        push_tool_exchange(messages, tc, assistant_text, &trimmed, remaining_provider_data.take());

        info!(tool = %name, result_len = result_str.len(), "Tool executed");
    }

    Ok(())
}

/// Push the assistant message (with tool_calls) and the tool result message.
fn push_tool_exchange(
    messages: &mut Vec<Message>,
    tc: &ToolCall,
    assistant_text: Option<&str>,
    result: &str,
    provider_data: Option<ProviderData>,
) {
    // Assistant message with tool call (provider_data carries thought_signatures for Gemini)
    messages.push(Message {
        role: Role::Assistant,
        content: MessageContent::Text(
            assistant_text.unwrap_or_default().to_string(),
        ),
        tool_calls: Some(vec![tc.clone()]),
        tool_call_id: None,
        provider_data,
    });

    // Tool result message
    messages.push(Message {
        role: Role::Tool,
        content: MessageContent::Text(result.to_string()),
        tool_calls: None,
        tool_call_id: Some(tc.id.clone()),
        provider_data: None,
    });
}

/// Trim tool results that are too large for the context window.
fn soft_trim(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let half = max_len / 2;
    format!(
        "{}\n\n... ({} chars trimmed) ...\n\n{}",
        &text[..half],
        text.len() - max_len,
        &text[text.len() - half..],
    )
}

#[derive(Debug, thiserror::Error)]
pub enum AgentLoopError {
    #[error("Too many consecutive errors: {0}")]
    TooManyErrors(String),
    #[error("Cancelled")]
    Cancelled,
    #[error("Runner error: {0}")]
    Runner(#[from] RunnerError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soft_trim_short() {
        let result = soft_trim("hello", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_soft_trim_long() {
        let text = "a".repeat(10000);
        let result = soft_trim(&text, 100);
        assert!(result.len() < text.len());
        assert!(result.contains("trimmed"));
    }
}
