use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::config::Config;
use crate::core::agent_loop;
use crate::core::memory::MemoryStore;
use crate::core::prompt;
use crate::core::runner::Runner;
use crate::core::sessions::SessionManager;
use crate::core::tools::{ToolContext, ToolRegistry};
use crate::core::types::{Message, MessageContent, Role};
use crate::core::workspace;
use crate::platform::http_client::HttpClient;

/// Core orchestrator. Holds config, tools, sessions, memory, and provides the chat() entry point.
pub struct Gateway {
    pub config: Arc<Config>,
    pub tools: ToolRegistry,
    pub sessions: Arc<SessionManager>,
    pub memory: Arc<Mutex<MemoryStore>>,
    pub runner: Runner,
    pub data_dir: String,
    /// Per-chat cancellation tokens — new message on a busy chat cancels the running turn.
    active_chats: Mutex<HashMap<String, CancellationToken>>,
}

impl Gateway {
    pub fn new(config: Config, data_dir: &str, http: Arc<dyn HttpClient>) -> Self {
        let sessions_dir = format!("{}/sessions", data_dir);
        let config = Arc::new(config);
        Self {
            runner: Runner::new(config.clone(), http),
            config: config.clone(),
            tools: ToolRegistry::new(),
            sessions: Arc::new(SessionManager::new(&sessions_dir)),
            memory: Arc::new(Mutex::new(MemoryStore::new(data_dir))),
            data_dir: data_dir.to_string(),
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
        // Cancel any running turn on this chat
        let cancel = {
            let mut active = self.active_chats.lock().await;
            if let Some(prev) = active.remove(chat_id) {
                prev.cancel();
            }
            let token = CancellationToken::new();
            active.insert(chat_id.to_string(), token.clone());
            token
        };

        info!(chat_id, channel, "Chat started");

        // Build system prompt
        let context_files = workspace::load_bootstrap_files(&self.data_dir);
        let tool_defs = self.tools.definitions();
        let system_prompt = prompt::build_system_prompt(
            &self.config,
            &tool_defs,
            &context_files,
            Some(channel),
            Some(chat_id),
        );

        // Load session history
        let branch = self
            .sessions
            .get_branch(chat_id)
            .map_err(|e| GatewayError::Session(e.to_string()))?;

        // Build messages
        let mut messages = Vec::new();

        // System prompt
        messages.push(Message {
            role: Role::System,
            content: MessageContent::Text(system_prompt),
            tool_calls: None,
            tool_call_id: None,
        });

        // Session history
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
                });
            }
        }

        // User message
        messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(message.to_string()),
            tool_calls: None,
            tool_call_id: None,
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
        self.sessions
            .append(chat_id, &user_entry)
            .map_err(|e| GatewayError::Session(e.to_string()))?;

        // Build tool context
        let ctx = ToolContext {
            chat_id: chat_id.to_string(),
            prompt_source: Some("chat".to_string()),
            config: self.config.clone(),
            sessions: self.sessions.clone(),
            data_dir: self.data_dir.clone(),
        };

        // Get model override from session state
        let model_override = self.sessions.get_state(chat_id).model_override;

        // Run agent loop
        let result = agent_loop::run_loop(
            &mut messages,
            &self.tools,
            &self.runner,
            &ctx,
            Some(&cancel),
            model_override.as_deref(),
        )
        .await
        .map_err(|e| GatewayError::AgentLoop(e.to_string()))?;

        // Persist assistant response to session
        let assistant_entry = crate::core::sessions::SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent: None,
            role: Role::Assistant,
            content: result.clone(),
            tool_calls: None,
            tool_call_id: None,
        };
        self.sessions
            .append(chat_id, &assistant_entry)
            .map_err(|e| GatewayError::Session(e.to_string()))?;

        // Remove from active chats
        self.active_chats.lock().await.remove(chat_id);

        Ok(result)
    }

    /// Cancel an active chat turn.
    pub async fn cancel_chat(&self, chat_id: &str) -> bool {
        let active = self.active_chats.lock().await;
        if let Some(token) = active.get(chat_id) {
            token.cancel();
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
