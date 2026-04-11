use crate::config::Config;
use crate::core::sessions::SessionManager;
use crate::core::tools::ToolRegistry;

/// Core orchestrator. Holds config, tools, sessions, and provides the chat() entry point.
pub struct Gateway {
    pub config: Config,
    pub tools: ToolRegistry,
    pub sessions: SessionManager,
}

impl Gateway {
    pub fn new(config: Config, sessions_dir: &str) -> Self {
        Self {
            config,
            tools: ToolRegistry::new(),
            sessions: SessionManager::new(sessions_dir),
        }
    }

    /// Main chat entry point.
    pub async fn chat(
        &self,
        _chat_id: &str,
        _message: &str,
        _channel: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: build prompt, run agent loop, return response
        todo!("Gateway::chat")
    }
}
