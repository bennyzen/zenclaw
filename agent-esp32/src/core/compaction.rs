//! Auto-compaction: when a session's persisted history grows past a token
//! or byte threshold, summarize the older entries into a single Compaction
//! entry and keep the most recent K messages verbatim.
//!
//! Triggered from `gateway::chat` before the message vector is rebuilt.
//! Failures are non-fatal: if the summarizer call errors, we log and skip,
//! so the user's turn still proceeds (they may then hit a context-too-long
//! at the provider, which is a softer failure than the agent crashing).

use log::{info, warn};
use std::time::Instant;

use crate::config::CompactionConfig;
use crate::core::runner::LlmRunner;
use crate::core::sessions::{SessionEntry, SessionManager};
use crate::core::types::{LlmResponse, Message, MessageContent, Role};

/// Pure predicate: should we compact this branch right now?
///
/// Token estimate is `len(content) + len(tool_calls_json)` summed across
/// all message entries, divided by 4 (the bytes-per-token proxy used by
/// the synthetic-session driver). Byte threshold compares against the
/// JSONL file's on-disk size, passed in by the caller.
pub fn should_compact(
    branch: &[SessionEntry],
    jsonl_bytes: usize,
    cfg: &CompactionConfig,
) -> bool {
    if !cfg.enabled {
        return false;
    }
    let message_count = branch
        .iter()
        .filter(|e| matches!(e, SessionEntry::Message { .. }))
        .count();
    if message_count <= cfg.keep_recent {
        return false;
    }
    if jsonl_bytes > cfg.byte_threshold {
        return true;
    }
    estimated_tokens(branch) > cfg.token_threshold
}

fn estimated_tokens(branch: &[SessionEntry]) -> usize {
    let mut bytes = 0usize;
    for e in branch {
        if let SessionEntry::Message {
            content,
            tool_calls,
            ..
        } = e
        {
            bytes += content.len();
            if let Some(tc) = tool_calls {
                if let Ok(s) = serde_json::to_string(tc) {
                    bytes += s.len();
                }
            }
        }
    }
    bytes / 4
}

/// Build the summarizer's input as a two-message conversation: a tight
/// system prompt instructing what to preserve, and a user message that
/// is the chronological transcript of the to-be-compacted entries.
fn build_summarizer_messages(
    to_summarize: &[SessionEntry],
    max_summary_bytes: usize,
) -> Vec<Message> {
    let system = format!(
        "You are compacting a long agent conversation so it can keep going \
        without exceeding the model's context window. Output a tight summary \
        in at most {max_summary_bytes} characters. Preserve, in this priority \
        order:\n\
        1. Pending user requests / unfinished tasks\n\
        2. Recent tool failures (so the agent does not repeat them)\n\
        3. Named entities the agent referred to: file paths, URLs, memory \
           keys, identifiers, numbers\n\
        4. The most recent user-stated topic or intent\n\
        Discard pleasantries, redundant successful tool calls, and \
        exploratory chatter. Output plain prose. Do not invent facts. Do not \
        use headers. The summary will replace the original transcript in the \
        agent's working memory."
    );

    let mut transcript = String::new();
    for entry in to_summarize {
        if let SessionEntry::Message {
            role,
            content,
            tool_calls,
            tool_call_id,
            ..
        } = entry
        {
            let role_label = match role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            transcript.push_str(role_label);
            transcript.push_str(": ");
            if let Some(id) = tool_call_id {
                transcript.push_str(&format!("[result for tool_call_id={}] ", id));
            }
            transcript.push_str(content);
            if let Some(tc) = tool_calls {
                for call in tc {
                    transcript.push_str(&format!(
                        "\n  -> tool_call: {}({})",
                        call.function.name, call.function.arguments
                    ));
                }
            }
            transcript.push('\n');
        }
    }

    vec![
        Message {
            role: Role::System,
            content: MessageContent::Text(system),
            tool_calls: None,
            tool_call_id: None,
            provider_data: None,
        },
        Message {
            role: Role::User,
            content: MessageContent::Text(transcript),
            tool_calls: None,
            tool_call_id: None,
            provider_data: None,
        },
    ]
}

/// Statistics returned from a successful compaction.
pub struct CompactionStats {
    pub replaced: usize,
    pub kept: usize,
    pub summary_bytes: usize,
    pub latency_ms: u128,
}

/// Run a compaction pass on `chat_id`'s session. Caller is responsible for
/// having checked `should_compact` first; this function does the work
/// unconditionally on the current branch.
///
/// On summarizer failure, returns Err — the caller logs and continues
/// without compacting. We deliberately do NOT fall back to silent
/// truncation of the history: if the LLM call fails, leaving the JSONL
/// untouched is the least-surprising behavior.
pub async fn compact_session(
    sessions: &SessionManager,
    runner: &dyn LlmRunner,
    chat_id: &str,
    cfg: &CompactionConfig,
    model_override: Option<&str>,
) -> Result<CompactionStats, String> {
    let start = Instant::now();
    let branch = sessions
        .get_branch(chat_id)
        .map_err(|e| format!("get_branch failed: {e}"))?;

    let total_msgs = branch
        .iter()
        .filter(|e| matches!(e, SessionEntry::Message { .. }))
        .count();
    if total_msgs <= cfg.keep_recent {
        return Err("nothing to compact".to_string());
    }
    let split = total_msgs - cfg.keep_recent;

    let mut to_summarize: Vec<SessionEntry> = Vec::with_capacity(split);
    let mut seen_msgs = 0usize;
    for entry in &branch {
        if matches!(entry, SessionEntry::Message { .. }) {
            if seen_msgs < split {
                to_summarize.push(entry.clone());
            }
            seen_msgs += 1;
        }
    }

    let prompt = build_summarizer_messages(&to_summarize, cfg.max_summary_bytes);
    let resp = runner
        .call(&prompt, &[], model_override)
        .await
        .map_err(|e| format!("summarizer LLM call failed: {e}"))?;

    let mut summary = match resp {
        LlmResponse::Text(s) => s,
        LlmResponse::Mixed { text, .. } => text,
        LlmResponse::ToolCalls { .. } => {
            return Err("summarizer returned tool_calls (no tools were offered)".to_string());
        }
    };

    if summary.len() > cfg.max_summary_bytes {
        let cut = nearest_char_boundary(&summary, cfg.max_summary_bytes);
        summary.truncate(cut);
    }

    sessions
        .compact(chat_id, &summary, cfg.keep_recent)
        .map_err(|e| format!("session compact failed: {e}"))?;

    let stats = CompactionStats {
        replaced: split,
        kept: cfg.keep_recent,
        summary_bytes: summary.len(),
        latency_ms: start.elapsed().as_millis(),
    };
    info!(
        "compaction: chat_id={} replaced={} kept={} summary_bytes={} latency_ms={}",
        chat_id, stats.replaced, stats.kept, stats.summary_bytes, stats.latency_ms
    );
    Ok(stats)
}

/// Top-level wrapper: check the predicate, run compaction if it fires.
/// Compaction failure is logged and swallowed — the caller's turn proceeds.
pub async fn maybe_compact(
    sessions: &SessionManager,
    runner: &dyn LlmRunner,
    chat_id: &str,
    cfg: &CompactionConfig,
    model_override: Option<&str>,
) {
    let branch = match sessions.get_branch(chat_id) {
        Ok(b) => b,
        Err(e) => {
            warn!("compaction skipped: get_branch failed: {e}");
            return;
        }
    };
    let jsonl_bytes = sessions.session_size_bytes(chat_id).unwrap_or(0);
    if !should_compact(&branch, jsonl_bytes, cfg) {
        return;
    }
    info!(
        "compaction trigger fired: chat_id={} jsonl_bytes={} est_tokens={} message_entries={}",
        chat_id,
        jsonl_bytes,
        estimated_tokens(&branch),
        branch
            .iter()
            .filter(|e| matches!(e, SessionEntry::Message { .. }))
            .count()
    );
    if let Err(e) = compact_session(sessions, runner, chat_id, cfg, model_override).await {
        warn!("compaction failed (continuing without): {e}");
    }
}

fn nearest_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Role;

    fn msg(role: Role, content: &str) -> SessionEntry {
        SessionEntry::Message {
            id: format!("m-{}", content.len()),
            parent: None,
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn cfg() -> CompactionConfig {
        CompactionConfig {
            enabled: true,
            token_threshold: 50_000,
            byte_threshold: 200 * 1024,
            keep_recent: 6,
            max_summary_bytes: 5 * 1024,
        }
    }

    #[test]
    fn predicate_disabled_never_fires() {
        let mut c = cfg();
        c.enabled = false;
        let branch = vec![msg(Role::User, &"x".repeat(1_000_000))];
        assert!(!should_compact(&branch, 999_999_999, &c));
    }

    #[test]
    fn predicate_below_keep_recent_never_fires() {
        let c = cfg();
        let branch: Vec<_> = (0..6).map(|_| msg(Role::User, &"x".repeat(1_000_000))).collect();
        assert!(!should_compact(&branch, 0, &c));
    }

    #[test]
    fn predicate_byte_threshold_fires() {
        let c = cfg();
        let branch: Vec<_> = (0..10).map(|_| msg(Role::User, "tiny")).collect();
        assert!(should_compact(&branch, 200 * 1024 + 1, &c));
    }

    #[test]
    fn predicate_token_threshold_fires() {
        let c = cfg();
        // 10 messages, each ~25 KB content = ~63K tokens estimated.
        let big = "x".repeat(25_000);
        let branch: Vec<_> = (0..10).map(|_| msg(Role::User, &big)).collect();
        assert!(should_compact(&branch, 0, &c));
    }

    #[test]
    fn predicate_under_both_thresholds_does_not_fire() {
        let c = cfg();
        let branch: Vec<_> = (0..10).map(|_| msg(Role::User, "small message")).collect();
        assert!(!should_compact(&branch, 50 * 1024, &c));
    }

    #[test]
    fn nearest_boundary_handles_multibyte() {
        // "héllo" — 'é' is 2 bytes (0xC3 0xA9) at index 1..3.
        let s = "héllo";
        // Cutting at 2 lands inside 'é' — should slide back to 1.
        assert_eq!(nearest_char_boundary(s, 2), 1);
        // Cutting beyond len — clamp.
        assert_eq!(nearest_char_boundary(s, 100), s.len());
    }
}
