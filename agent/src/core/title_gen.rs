//! Background-style task that upgrades a chat's title from a
//! `FirstMessage`/`Default` source to an LLM-summarized one.
//!
//! Runs *after* the user has received their reply (the `ChatEvent::Done`
//! event has already been dispatched on the WS path) — adds latency only
//! to the function-return for REST/Telegram callers. The trigger
//! condition (`title_source != User && != Llm`) re-arms on the next turn
//! so a transient LLM outage just defers the title upgrade.

use crate::core::runner::LlmRunner;
use crate::core::sessions::meta::TitleSource;
use crate::core::sessions::{SessionEntry, SessionManager};
use crate::core::types::{LlmResponse, Message, MessageContent, Role};

const TITLE_PROMPT: &str = "Summarize the user's question in 6 words or fewer. \
Output only the title — no quotes, no punctuation, no preamble.";

/// One-shot LLM call to summarize this chat into a sidebar title.
/// Bails out unless the meta's `title_source` is `Default` or
/// `FirstMessage`. On success, calls `rename_internal(..., Llm)`.
/// Failures are logged via `tracing::warn!` and not propagated.
pub async fn maybe_generate_title(
    chat_id: &str,
    sessions: &SessionManager,
    runner: &dyn LlmRunner,
) {
    // Bail when not needed: User-renamed or already LLM-titled.
    let meta = match sessions.meta(chat_id) {
        Ok(Some(m)) => m,
        _ => return,
    };
    match meta.title_source {
        TitleSource::Llm | TitleSource::User => return,
        TitleSource::Default | TitleSource::FirstMessage => {}
    }

    // Build the prompt: last 6 message-typed entries + the title prompt
    // as a system message at the front.
    let entries = match sessions.load(chat_id) {
        Ok(es) => es,
        Err(_) => return,
    };
    let context: Vec<Message> = entries
        .into_iter()
        .filter_map(|e| match e {
            SessionEntry::Message { role, content, .. } => Some(Message {
                role,
                content: MessageContent::Text(content),
                tool_calls: None,
                tool_call_id: None,
                provider_data: None,
            }),
            _ => None,
        })
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if context.is_empty() {
        return;
    }
    let mut messages = Vec::with_capacity(context.len() + 1);
    messages.push(Message {
        role: Role::System,
        content: MessageContent::Text(TITLE_PROMPT.to_string()),
        tool_calls: None,
        tool_call_id: None,
        provider_data: None,
    });
    messages.extend(context);

    // One-shot LLM call. No tools. Default model.
    let response = match runner.call(&messages, &[], None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("title_gen for {}: LLM call failed: {}", chat_id, e);
            return;
        }
    };

    // Extract text from the response. Tool-only responses are unexpected
    // (we passed empty tools) — log and skip.
    let raw = match response {
        LlmResponse::Text(t) => t,
        LlmResponse::Mixed { text, .. } => text,
        LlmResponse::ToolCalls { .. } => {
            tracing::warn!("title_gen for {}: unexpected tool-only response", chat_id);
            return;
        }
    };

    let title = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string();
    if title.is_empty() || title.chars().count() > 80 {
        tracing::warn!(
            "title_gen for {}: rejected title (length {})",
            chat_id,
            title.chars().count()
        );
        return;
    }

    if let Err(e) = sessions.rename_internal(chat_id, &title, TitleSource::Llm) {
        tracing::warn!("title_gen rename_internal for {}: {}", chat_id, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runner::RunnerError;
    use crate::core::sessions::meta::SessionMeta;
    use crate::core::types::ToolDefinition;
    use async_trait::async_trait;

    struct PanicRunner;

    #[async_trait]
    impl LlmRunner for PanicRunner {
        async fn call(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model_override: Option<&str>,
        ) -> Result<LlmResponse, RunnerError> {
            panic!("LLM should not be called when title_source is User or Llm");
        }
    }

    #[tokio::test]
    async fn bails_when_title_source_is_user() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let mut m = SessionMeta::synthesize_default("chat-1", 100, None);
        m.title_source = TitleSource::User;
        mgr.set_meta("chat-1", &m).unwrap();

        // Should not panic — the function bails before calling runner.
        maybe_generate_title("chat-1", &mgr, &PanicRunner).await;
    }

    #[tokio::test]
    async fn bails_when_title_source_is_llm() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let mut m = SessionMeta::synthesize_default("chat-1", 100, Some("hello"));
        m.title_source = TitleSource::Llm;
        mgr.set_meta("chat-1", &m).unwrap();

        // Should not panic — the function bails before calling runner.
        maybe_generate_title("chat-1", &mgr, &PanicRunner).await;
    }

    #[tokio::test]
    async fn bails_when_meta_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        // No prior set_meta.
        maybe_generate_title("nonexistent", &mgr, &PanicRunner).await;
    }
}
