use std::sync::atomic::{AtomicBool, Ordering};
use log::{info, error};

use crate::core::runner::{LlmRunner, RunnerError};
use crate::core::tool_loop::{LoopDetector, LoopLevel};
use crate::core::tools::{ToolContext, ToolRegistry};
use crate::core::types::{LlmResponse, Message, MessageContent, ProviderData, Role, ToolCall};

const MAX_CONSECUTIVE_ERRORS: usize = 3;

/// Maximum size of a single tool result, in bytes, before we refuse it
/// back to the LLM with a "narrow your scope" error. Desktop is uncapped
/// so we can observe what real tool outputs look like; ESP32 is sized
/// for comfortable PSRAM headroom (DevKitC 8MB, P4 32MB).
#[cfg(feature = "desktop")]
const MAX_TOOL_RESULT_BYTES: usize = usize::MAX;
#[cfg(not(feature = "desktop"))]
const MAX_TOOL_RESULT_BYTES: usize = 256 * 1024;

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
    runner: &dyn LlmRunner,
    ctx: &ToolContext,
    cancel: Option<&AtomicBool>,
    model_override: Option<&str>,
) -> Result<String, AgentLoopError> {
    let tool_defs = tools.definitions();
    let mut consecutive_errors: usize = 0;
    let mut loop_detector = LoopDetector::new();
    let mut in_tool_loop = false;
    loop {
        // Check cancellation before LLM call
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                info!("Agent loop cancelled before LLM call");
                return Ok("Operation cancelled.".to_string());
            }
        }

        // After tool execution, skip tool schemas to reduce payload size.
        // On memory-constrained devices (ESP32 without PSRAM), the full tool
        // schemas add ~4-6KB to the JSON payload which can cause OOM on the
        // second TLS+serialize pass. The LLM already saw tools on the first
        // call and just needs to process tool results now.
        let effective_tools: &[_] = if in_tool_loop { &[] } else { &tool_defs };

        // Call LLM
        let response = match runner.call(messages, effective_tools, model_override).await {
            Ok(r) => {
                consecutive_errors = 0;
                r
            }
            Err(e) => {
                consecutive_errors += 1;
                error!("LLM call failed (attempt {}): {}", consecutive_errors, e);
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    return Err(AgentLoopError::TooManyErrors(e.to_string()));
                }
                continue;
            }
        };

        match response {
            LlmResponse::Text(text) => {
                messages.push(Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(text.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    provider_data: None,
                });
                return Ok(text);
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
                in_tool_loop = true;
            }
            LlmResponse::Mixed { text, tool_calls, provider_data } => {
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
                in_tool_loop = true;
            }
        }
    }
}

async fn execute_tool_calls(
    tool_calls: &[ToolCall],
    assistant_text: Option<&str>,
    provider_data: Option<ProviderData>,
    messages: &mut Vec<Message>,
    tools: &ToolRegistry,
    ctx: &ToolContext,
    cancel: Option<&AtomicBool>,
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
                info!("Loop detector blocked tool call: {} ({})", name, check.detector);
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
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                push_tool_exchange(messages, tc, assistant_text, "Skipped: operation cancelled.", remaining_provider_data.take());
                info!("Tool skipped — cancelled: {}", name);
                continue;
            }
        }

        loop_detector.record_call(name, &args);

        // Execute the tool
        let result = tools.execute(name, args.clone(), ctx).await;
        let result_str = result.to_string();
        let result_len = result_str.len();

        loop_detector.record_outcome(name, &args, &result_str);

        // Cap or refuse — never destructively truncate. Past behavior was a
        // head+tail split that mangled structured outputs (JSON / markdown /
        // code) into syntactic garbage halfway through. The LLM then quoted
        // that garbage back hallucinated. The new rule: if the result is too
        // big, the LLM gets a clear error it can act on (narrow scope,
        // paginate, etc.) instead of corrupted bytes.
        let payload = cap_or_refuse(result_str, MAX_TOOL_RESULT_BYTES);

        push_tool_exchange(messages, tc, assistant_text, &payload, remaining_provider_data.take());

        info!(
            "Tool executed: {} (raw={}B, sent={}B)",
            name, result_len, payload.len()
        );
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

/// Return `result` unchanged if it fits, otherwise return an actionable
/// error message naming the actual size and cap so the LLM can narrow scope
/// on retry. Never truncates; preservation over destruction.
fn cap_or_refuse(result: String, cap: usize) -> String {
    if result.len() <= cap {
        return result;
    }
    format!(
        "Tool result was {} bytes, exceeded the {} byte cap. Re-run with \
         narrower scope (paginated read, smaller result limit, or more \
         specific query) so the response fits.",
        result.len(),
        cap,
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
    fn cap_or_refuse_passes_under_cap_unchanged() {
        let result = cap_or_refuse("hello".to_string(), 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn cap_or_refuse_passes_at_cap_unchanged() {
        let result = cap_or_refuse("a".repeat(100), 100);
        assert_eq!(result.len(), 100);
        assert!(result.chars().all(|c| c == 'a'));
    }

    #[test]
    fn cap_or_refuse_over_cap_returns_actionable_error() {
        let result = cap_or_refuse("x".repeat(200), 100);
        // Names the actual size, the cap, and what to do — no garbage payload.
        assert!(result.contains("200 bytes"));
        assert!(result.contains("100 byte cap"));
        assert!(result.contains("narrower scope"));
        // Crucially: original bytes are NOT in the error message.
        assert!(!result.contains("xxx"));
    }
}
