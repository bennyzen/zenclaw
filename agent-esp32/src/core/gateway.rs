use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use log::info;

use crate::config::Config;
use crate::core::agent_loop;
use crate::core::memory::MemoryStore;
use crate::core::prompt;
use crate::core::runner::LlmRunner;
use crate::core::sessions::SessionManager;
use crate::core::tools::{ToolContext, ToolRegistry};
use crate::core::types::{Message, MessageContent, Role};
use crate::core::workspace;

/// Core orchestrator. Holds config, tools, sessions, memory, and provides the chat() entry point.
pub struct Gateway {
    pub config: Arc<Config>,
    pub tools: ToolRegistry,
    pub sessions: Arc<SessionManager>,
    pub memory: Arc<Mutex<MemoryStore>>,
    pub runner: Box<dyn LlmRunner>,
    pub data_dir: String,
    /// Per-chat cancellation flags — new message on a busy chat cancels the running turn.
    active_chats: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Gateway {
    pub fn new(config: Config, data_dir: &str, runner: Box<dyn LlmRunner>) -> Self {
        let sessions_dir = format!("{}/sessions", data_dir);
        let config = Arc::new(config);
        let mut tools = ToolRegistry::new();
        tools.register_defaults();
        Self {
            runner,
            config: config.clone(),
            tools,
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
            let mut active = self.active_chats.lock().unwrap();
            if let Some(prev) = active.remove(chat_id) {
                prev.store(true, Ordering::Relaxed);
            }
            let flag = Arc::new(AtomicBool::new(false));
            active.insert(chat_id.to_string(), flag.clone());
            flag
        };

        info!("GW chat: id={} ch={}", chat_id, channel);

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
        let ctx = ToolContext {
            chat_id: chat_id.to_string(),
            prompt_source: Some("chat".to_string()),
            config: self.config.clone(),
            sessions: self.sessions.clone(),
            data_dir: self.data_dir.clone(),
        };

        // Get model override from session state
        let model_override = self.sessions.get_state(chat_id).model_override;

        info!("GW: agent_loop msgs={}", messages.len());

        // Run agent loop
        let result = agent_loop::run_loop(
            &mut messages,
            &self.tools,
            self.runner.as_ref(),
            &ctx,
            Some(cancel.as_ref()),
            model_override.as_deref(),
        )
        .await
        .map_err(|e| GatewayError::AgentLoop(e.to_string()))?;

        info!("GW: reply {}B", result.len());

        // Persist assistant response to session
        let assistant_entry = crate::core::sessions::SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent: None,
            role: Role::Assistant,
            content: result.clone(),
            tool_calls: None,
            tool_call_id: None,
        };
        if let Err(e) = self.sessions.append(chat_id, &assistant_entry) {
            log::warn!("Session append failed (continuing): {}", e);
        }

        // Remove from active chats
        self.active_chats.lock().unwrap().remove(chat_id);

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
