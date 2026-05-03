use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use log::info;

use crate::config::Config;
use crate::core::agent_loop;
use crate::core::chat_events::{try_send, ChatEvent, Sender as EventSender};
use crate::core::cloud::client::ObjectStore;
use crate::core::cloud::{CloudCache, Replicator};
use crate::core::compaction;
use crate::core::prompt;
use crate::core::runner::LlmRunner;
use crate::core::sessions::SessionManager;
use crate::core::tools::{CloudToolHandles, ToolContext, ToolRegistry};
use crate::core::types::{ContentPart, Message, MessageContent, Role};
use crate::core::workspace;

/// Optional cloud-persistence handles passed to [`Gateway::new_with_cloud`].
/// `cache` and `replicator` are wired into the inner `SessionManager`;
/// they're also exposed on the Gateway so HTTP handlers can read sync
/// state (`/api/status.cloud_storage`) and surface dead-letter entries.
pub struct CloudHandles {
    pub cache: CloudCache,
    pub replicator: Arc<Replicator>,
    pub store: Arc<dyn ObjectStore>,
    pub log_compaction_bytes: usize,
    pub retry_max: u8,
    pub backoff_cap_secs: u32,
}

/// Core orchestrator. Holds config, tools, sessions, and provides the chat() entry point.
/// Memory is file-backed (data/MEMORY.md) and accessed via memory_* tools — no in-process state.
pub struct Gateway {
    pub config: Arc<Config>,
    pub tools: ToolRegistry,
    pub sessions: Arc<SessionManager>,
    pub runner: Box<dyn LlmRunner>,
    pub data_dir: String,
    /// Cloud Tier-1 cache. `None` → local-file mode. `Some` → SessionManager
    /// and tool handles (memory_tools etc.) read/write through here.
    pub cloud_cache: Option<CloudCache>,
    /// Eager-path replicator. Always `Some` whenever `cloud_cache` is.
    pub cloud_replicator: Option<Arc<Replicator>>,
    /// Direct S3 store for strict-path tool writes (memory/cron/config).
    pub cloud_store: Option<Arc<dyn ObjectStore>>,
    /// Strict-path retry policy (mirrors `replicator.retry_max` /
    /// `backoff_cap_secs` from `StorageConfig`).
    pub cloud_retry_max: u8,
    pub cloud_backoff_cap_secs: u32,
    /// Per-chat cancellation flags — new message on a busy chat cancels the running turn.
    active_chats: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Gateway {
    pub fn new(config: Config, data_dir: &str, runner: Box<dyn LlmRunner>) -> Self {
        Self::new_inner(config, data_dir, runner, None)
    }

    /// Cloud-aware constructor. Builds the inner [`SessionManager`] with
    /// `with_cloud(cache, replicator, log_compaction_bytes)` so session
    /// appends route through the cache + eager replicator instead of the
    /// local FS. Caller (typically `main.rs`) is responsible for spawning
    /// the replicator drainer and the snapshot timer.
    pub fn new_with_cloud(
        config: Config,
        data_dir: &str,
        runner: Box<dyn LlmRunner>,
        cloud: CloudHandles,
    ) -> Self {
        Self::new_inner(config, data_dir, runner, Some(cloud))
    }

    fn new_inner(
        config: Config,
        data_dir: &str,
        runner: Box<dyn LlmRunner>,
        cloud: Option<CloudHandles>,
    ) -> Self {
        let sessions_dir = format!("{}/sessions", data_dir);
        let config = Arc::new(config);
        let mut tools = ToolRegistry::new();
        tools.register_defaults();

        let mut session_mgr = SessionManager::new(&sessions_dir);
        let (cloud_cache, cloud_replicator, cloud_store, cloud_retry_max, cloud_backoff_cap_secs) =
            match cloud {
                Some(h) => {
                    session_mgr = session_mgr.with_cloud(
                        h.cache.clone(),
                        h.replicator.clone(),
                        h.log_compaction_bytes,
                    );
                    (
                        Some(h.cache),
                        Some(h.replicator),
                        Some(h.store),
                        h.retry_max,
                        h.backoff_cap_secs,
                    )
                }
                None => (None, None, None, 0, 0),
            };

        Self {
            runner,
            config: config.clone(),
            tools,
            sessions: Arc::new(session_mgr),
            data_dir: data_dir.to_string(),
            cloud_cache,
            cloud_replicator,
            cloud_store,
            cloud_retry_max,
            cloud_backoff_cap_secs,
            active_chats: Mutex::new(HashMap::new()),
        }
    }

    /// Main chat entry point.
    pub async fn chat(
        &self,
        chat_id: &str,
        message: &str,
        channel: &str,
    ) -> Result<String, GatewayError> {
        self.chat_with_events(chat_id, message, channel, None).await
    }

    /// Variant of `chat` that streams typed events to a caller-supplied
    /// channel. The REST entry point passes `None`; the WS handler passes a
    /// sender whose receiver runs on a forwarder thread that converts each
    /// event into a JSON WebSocket frame.
    pub async fn chat_with_events(
        &self,
        chat_id: &str,
        message: &str,
        channel: &str,
        events: Option<&EventSender>,
    ) -> Result<String, GatewayError> {
        // Cancel any running turn on this chat
        let cancel = {
            let mut active = self.active_chats.lock().unwrap();
            if let Some(prev) = active.remove(chat_id) {
                prev.store(true, Ordering::Relaxed);
            }
            let flag = Arc::new(AtomicBool::new(false));
            active.insert(chat_id.to_string(), flag.clone());
            flag
        };

        info!("GW chat: id={} ch={}", chat_id, channel);

        // Auto-compaction: before reading session history into the message
        // vector, summarize older entries if the branch has grown past the
        // configured token/byte thresholds. Failures here are non-fatal —
        // the user's turn proceeds with whatever history is on disk.
        let model_override = self.sessions.get_state(chat_id).model_override;
        compaction::maybe_compact(
            self.sessions.as_ref(),
            self.runner.as_ref(),
            chat_id,
            &self.config.compaction,
            model_override.as_deref(),
        )
        .await;

        // Build system prompt with all tools and context files
        let context_files = workspace::load_bootstrap_files(&self.data_dir);
        let tool_defs = self.tools.definitions();
        let system_prompt = prompt::build_system_prompt(
            &self.config,
            &tool_defs,
            &context_files,
            Some(channel),
            Some(chat_id),
        );
        drop(context_files);
        drop(tool_defs);

        // Build messages — on low memory, skip session history entirely
        let mut messages = Vec::new();

        messages.push(Message {
            role: Role::System,
            content: MessageContent::Text(system_prompt),
            tool_calls: None,
            tool_call_id: None,
            provider_data: None,
        });

        {
            let branch = self
                .sessions
                .get_branch(chat_id)
                .map_err(|e| GatewayError::Session(e.to_string()))?;

            for entry in &branch {
                if let crate::core::sessions::SessionEntry::Message {
                    role,
                    content,
                    tool_calls,
                    tool_call_id,
                    ..
                } = entry
                {
                    messages.push(Message {
                        role: role.clone(),
                        content: MessageContent::Text(content.clone()),
                        tool_calls: tool_calls.clone(),
                        tool_call_id: tool_call_id.clone(),
                        provider_data: None,
                    });
                }
            }
        }

        messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(message.to_string()),
            tool_calls: None,
            tool_call_id: None,
            provider_data: None,
        });

        // Persist user message to session
        let user_entry = crate::core::sessions::SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent: None,
            role: Role::User,
            content: message.to_string(),
            tool_calls: None,
            tool_call_id: None,
        };
        if let Err(e) = self.sessions.append(chat_id, &user_entry) {
            log::warn!("Session append failed (continuing): {}", e);
        }

        // Build tool context
        let cloud = match (&self.cloud_cache, &self.cloud_store) {
            (Some(cache), Some(store)) => Some(CloudToolHandles {
                cache: cache.clone(),
                store: store.clone(),
                retry_max: self.cloud_retry_max,
                backoff_cap_secs: self.cloud_backoff_cap_secs,
            }),
            _ => None,
        };
        let ctx = ToolContext {
            chat_id: chat_id.to_string(),
            prompt_source: Some("chat".to_string()),
            config: self.config.clone(),
            sessions: self.sessions.clone(),
            data_dir: self.data_dir.clone(),
            cloud,
        };

        // Capture the message count before the loop runs so we can persist
        // exactly the new messages it appends — assistant-with-tool-calls,
        // tool results, and the final assistant reply. Pre-loop messages
        // are either system (rebuilt next turn) or already in the session.
        let pre_loop_msg_count = messages.len();

        info!("GW: agent_loop msgs={}", messages.len());

        // Run agent loop
        let loop_result = agent_loop::run_loop(
            &mut messages,
            &self.tools,
            self.runner.as_ref(),
            &ctx,
            Some(cancel.as_ref()),
            model_override.as_deref(),
            events,
        )
        .await;

        let result = match loop_result {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                try_send(
                    events,
                    ChatEvent::Error {
                        error: err_str.clone(),
                    },
                );
                self.active_chats.lock().unwrap().remove(chat_id);
                return Err(GatewayError::AgentLoop(err_str));
            }
        };

        info!("GW: reply {}B", result.len());

        // Persist every new message the loop produced — including each
        // tool-call exchange. Without this, the session JSONL only carried
        // user/assistant text pairs, and the LLM had amnesia about its own
        // tool history across turns. That surfaced as redundant tool calls
        // (state desync). The schema already supported tool_calls and
        // tool_call_id; the gateway just wasn't writing them.
        let mut persisted = 0usize;
        for msg in &messages[pre_loop_msg_count..] {
            // Skip injected system warnings (circuit-breaker advisories) — they
            // were transient guidance for this turn, not durable history.
            if matches!(msg.role, Role::System) {
                continue;
            }
            let entry = crate::core::sessions::SessionEntry::Message {
                id: uuid::Uuid::new_v4().to_string(),
                parent: None,
                role: msg.role.clone(),
                content: message_content_as_text(&msg.content),
                tool_calls: msg.tool_calls.clone(),
                tool_call_id: msg.tool_call_id.clone(),
            };
            if let Err(e) = self.sessions.append(chat_id, &entry) {
                log::warn!("Session append failed (continuing): {}", e);
            } else {
                persisted += 1;
            }
        }
        log::debug!("GW: persisted {} new session entries", persisted);

        // Remove from active chats
        self.active_chats.lock().unwrap().remove(chat_id);

        try_send(events, ChatEvent::Done);

        Ok(result)
    }

    /// Cancel an active chat turn.
    pub async fn cancel_chat(&self, chat_id: &str) -> bool {
        let active = self.active_chats.lock().unwrap();
        if let Some(flag) = active.get(chat_id) {
            flag.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("Session error: {0}")]
    Session(String),
    #[error("Agent loop error: {0}")]
    AgentLoop(String),
}

/// Flatten MessageContent for JSONL persistence. Image parts are dropped
/// (the bytes can't round-trip through plain text); text parts are joined
/// with newlines so order is preserved.
fn message_content_as_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::ImageUrl { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ImageUrl;

    #[test]
    fn message_content_text_passthrough() {
        let c = MessageContent::Text("hello".to_string());
        assert_eq!(message_content_as_text(&c), "hello");
    }

    #[test]
    fn message_content_parts_flattens_text_drops_images() {
        let c = MessageContent::Parts(vec![
            ContentPart::Text { text: "first".to_string() },
            ContentPart::ImageUrl { image_url: ImageUrl { url: "data:...".to_string() } },
            ContentPart::Text { text: "second".to_string() },
        ]);
        assert_eq!(message_content_as_text(&c), "first\nsecond");
    }

    #[test]
    fn message_content_parts_only_image_yields_empty() {
        let c = MessageContent::Parts(vec![
            ContentPart::ImageUrl { image_url: ImageUrl { url: "x".to_string() } },
        ]);
        assert_eq!(message_content_as_text(&c), "");
    }
}
