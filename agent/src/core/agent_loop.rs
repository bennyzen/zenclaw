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
    let mut tool_calls_this_turn: usize = 0;
    let mut tool_names_this_turn: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut continuation_nudges: usize = 0;
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

                // The LLM sometimes narrates next steps ("Let me read each
                // one..." ending in a colon or bare announcement) and stops
                // emitting tool_calls — leaving multi-step work undone. If
                // we've already executed at least one tool this turn and
                // the final text looks like an unkept promise, push a system
                // reminder and re-call the LLM. Limit to MAX_CONTINUATION_NUDGES
                // per turn to avoid loops on edge cases.
                if tool_calls_this_turn > 0
                    && continuation_nudges < MAX_CONTINUATION_NUDGES
                    && looks_like_incomplete_narrative(&text, &tool_names_this_turn)
                {
                    continuation_nudges += 1;
                    info!(
                        "Continuation nudge {}/{}: text looks incomplete after {} tool calls",
                        continuation_nudges, MAX_CONTINUATION_NUDGES, tool_calls_this_turn
                    );
                    messages.push(Message {
                        role: Role::System,
                        content: MessageContent::Text(
                            "REMINDER: your previous response did not advance the task. \
                             Either you described next steps but stopped emitting \
                             tool_calls, OR you wrote tool-call markup (<tool_call ...>) \
                             as TEXT instead of using the proper tool_calls JSON schema. \
                             Continue the task NOW by emitting a proper tool_calls field \
                             on your next response — do not put tool calls inside the \
                             content string. Only return a final text response when every \
                             step is actually done."
                                .to_string(),
                        ),
                        tool_calls: None,
                        tool_call_id: None,
                        provider_data: None,
                    });
                    // Re-include tool schemas so the LLM can call them.
                    in_tool_loop = false;
                    continue;
                }

                return Ok(text);
            }
            LlmResponse::ToolCalls { tool_calls, provider_data } => {
                tool_calls_this_turn += tool_calls.len();
                for tc in &tool_calls {
                    tool_names_this_turn.insert(tc.function.name.clone());
                }
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
                tool_calls_this_turn += tool_calls.len();
                for tc in &tool_calls {
                    tool_names_this_turn.insert(tc.function.name.clone());
                }
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

/// Cap on continuation nudges per single chat turn — prevents nudge loops
/// on pathological assistants that always emit narrative-style replies.
/// Sized for compound prompts that need multiple chained tool calls
/// (e.g. "find every X, do Y to each, then Z" — 4+ steps is realistic).
const MAX_CONTINUATION_NUDGES: usize = 4;

/// Heuristic: does this assistant text look like it stopped mid-task,
/// promising more work but not actually doing it? We only consult this
/// when the LLM has already executed at least one tool this turn, so
/// every check is in the context of "should we let it finish?"
///
/// `tools_called` is the set of tool names already invoked this turn.
/// We use it to detect "I'll save these to memory" prose that's
/// followed by inline content but never an actual `memory` call.
///
/// Triggers on:
/// - Trailing colon, ellipsis, or unfinished markdown markers
/// - Inline tool-call markup (`<tool_call ...>`, `<parameter ...>`) which
///   means the model intended to call a tool but rendered it as text
///   instead of using the proper tool_calls JSON schema. Observed
///   repeatedly with z.ai glm-5.1 on chained multi-call turns.
/// - Action-narrative phrases ("Let me", "I'll now", "Next, I'll")
///   appearing in the trailing window of the response
/// - "Save/store/remember it" promises about a tool that wasn't called
fn looks_like_incomplete_narrative(
    text: &str,
    tools_called: &std::collections::HashSet<String>,
) -> bool {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        // An empty Text response after tool calls is itself a non-answer.
        return true;
    }

    // Inline tool-call markup as text content — the model tried to call
    // a tool but used the wrong format. A textual <tool_call> never
    // executes; we need to nudge the model to retry with proper tool_calls.
    if trimmed.contains("<tool_call")
        || trimmed.contains("</tool_call")
        || trimmed.contains("<parameter")
    {
        return true;
    }

    // Trailing punctuation that almost always means "continuing".
    let last_char = trimmed.chars().last();
    if matches!(last_char, Some(':')) {
        return true;
    }
    if trimmed.ends_with("...") || trimmed.ends_with("…") {
        return true;
    }
    // Bare bold markdown announcement at the very end ("**NextStep**")
    if trimmed.ends_with("**") {
        // Check it's a closing bold marker (preceded by content), not
        // an opening that the model already handles.
        let no_trail = &trimmed[..trimmed.len() - 2];
        if no_trail.contains("**") {
            return true;
        }
    }

    // Action-narrative phrase appearing in the SUFFIX of the response.
    // Looking at the trailing window (not the whole text) — narrative
    // phrases at the start of a long response are normal explanation;
    // the same phrases near the end are unkept promises. Suffix-based
    // because per-sentence parsing is fragile around filenames like
    // "AGENTS.md" where periods look like sentence terminators.
    let suffix = trailing_window(trimmed, NARRATIVE_SUFFIX_BYTES);
    let lower = suffix.to_ascii_lowercase();
    const NARRATIVE_PREFIXES: &[&str] = &[
        "let me ",
        "i'll ",
        "i will ",
        "now let me ",
        "now i'll ",
        "now i will ",
        "next, i'll ",
        "next i'll ",
        "next, let me ",
        "first, let me ",
        "first i'll ",
        "let's ",
    ];
    for p in NARRATIVE_PREFIXES {
        if lower.contains(p) {
            return true;
        }
    }

    // Promised-tool-action-but-tool-not-called check. If the response
    // talks about saving / storing / writing to memory but the `memory`
    // tool was never invoked this turn, the model emitted prose where
    // it should have emitted a tool call.
    let full_lower = text.to_ascii_lowercase();
    if !tools_called.contains("memory") {
        const MEMORY_SAVE_PROMISES: &[&str] = &[
            "save them as memory",
            "save these as memory",
            "save it as memory",
            "save them to memory",
            "save these to memory",
            "save it to memory",
            "store them as memory",
            "store these as memory",
            "store it as memory",
            "write them to memory",
            "write these to memory",
            "save the summaries",
            "save the summary",
            "save as memory entries",
            "save as a memory entry",
            "save them as separate memory",
            "save each as a memory",
        ];
        for p in MEMORY_SAVE_PROMISES {
            if full_lower.contains(p) {
                return true;
            }
        }
    }

    false
}

/// How much of the trailing text to inspect for action-narrative phrases.
/// Tuned for typical reply lengths; long enough to catch a multi-clause
/// "Let me also check X, then read Y" but short enough that earlier
/// explanation doesn't false-positive.
const NARRATIVE_SUFFIX_BYTES: usize = 200;

fn trailing_window(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
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

    fn empty_tools() -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
    }

    fn tools_with(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn narrative_trailing_colon_caught() {
        assert!(looks_like_incomplete_narrative(
            "Three .md files found. Let me read each one:",
            &empty_tools(),
        ));
    }

    #[test]
    fn narrative_let_me_in_last_sentence_caught() {
        assert!(looks_like_incomplete_narrative(
            "Found the files. Let me check the contents of each.",
            &empty_tools(),
        ));
    }

    #[test]
    fn narrative_ellipsis_caught() {
        assert!(looks_like_incomplete_narrative("Now reading the files...", &empty_tools()));
        assert!(looks_like_incomplete_narrative("Now reading the files…", &empty_tools()));
    }

    #[test]
    fn narrative_completed_response_passes() {
        assert!(!looks_like_incomplete_narrative(
            "All three .md files have been summarized into memory entries: \
             summary_AGENTS, summary_MEMORY, summary_SOUL. Done.",
            &tools_with(&["file", "memory"]),
        ));
    }

    #[test]
    fn narrative_explanatory_let_me_earlier_does_not_trigger() {
        let early = "let me try a few things. ".to_string();
        let padding = "Successfully completed the task. ".repeat(10);
        let text = format!("Earlier I said {}{}", early, padding);
        assert!(text.len() > NARRATIVE_SUFFIX_BYTES);
        assert!(!looks_like_incomplete_narrative(&text, &tools_with(&["memory"])));
    }

    #[test]
    fn narrative_let_me_after_md_filenames_caught() {
        assert!(looks_like_incomplete_narrative(
            "The data/ directory only has scratch-v2.log. But I see .md files in the root: AGENTS.md, MEMORY.md, SOUL.md. Let me also check memory/ for .md files, and then read all of them.",
            &empty_tools(),
        ));
    }

    #[test]
    fn narrative_empty_after_tools_caught() {
        assert!(looks_like_incomplete_narrative("", &empty_tools()));
        assert!(looks_like_incomplete_narrative("   \n\n  ", &empty_tools()));
    }

    #[test]
    fn narrative_bare_bold_announcement_caught() {
        assert!(looks_like_incomplete_narrative(
            "Three files found. **AGENTS.md**",
            &empty_tools(),
        ));
    }

    #[test]
    fn narrative_inline_tool_call_markup_caught() {
        assert!(looks_like_incomplete_narrative(
            "Reading the files. <tool_call tool=\"file\" action=\"read\" path=\"AGENTS.md\"></tool_call>",
            &empty_tools(),
        ));
        assert!(looks_like_incomplete_narrative(
            "<parameter name=\"path\">SOUL.md</parameter>",
            &empty_tools(),
        ));
    }

    #[test]
    fn promise_to_save_memory_without_tool_call_caught() {
        // The model says it'll save the summaries to memory but only
        // writes them as inline text — memory tool was never called.
        let text = "Now I have read all three files. Let me create one-line summaries for each and save them as memory entries.\n\n- AGENTS.md: \"foo\"\n- MEMORY.md: \"bar\"\n- SOUL.md: \"baz\"";
        assert!(looks_like_incomplete_narrative(text, &tools_with(&["file"])));
    }

    #[test]
    fn promise_to_save_memory_with_tool_call_passes() {
        // Same prose but with memory.save actually called → not incomplete.
        let text = "Saved all three summaries to memory: summary_AGENTS, summary_MEMORY, summary_SOUL.";
        assert!(!looks_like_incomplete_narrative(
            text,
            &tools_with(&["file", "memory"])
        ));
    }
}
