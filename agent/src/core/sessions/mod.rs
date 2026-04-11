use serde::{Deserialize, Serialize};

use crate::core::types::Message;

/// A single entry in a session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "message")]
    Message {
        message: Message,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },
    #[serde(rename = "summary")]
    Summary {
        text: String,
        turn_count: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },
    #[serde(rename = "branch")]
    Branch {
        parent_id: String,
        branch_id: String,
    },
}

/// Manages JSONL-based conversation sessions.
pub struct SessionManager {
    sessions_dir: String,
}

impl SessionManager {
    pub fn new(sessions_dir: &str) -> Self {
        Self {
            sessions_dir: sessions_dir.to_string(),
        }
    }

    /// Load conversation history for a chat.
    pub fn load(&self, _chat_id: &str) -> Result<Vec<Message>, Box<dyn std::error::Error>> {
        // TODO: implement JSONL loading
        Ok(Vec::new())
    }

    /// Append a message to the session.
    pub fn append(
        &self,
        _chat_id: &str,
        _entry: &SessionEntry,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // TODO: implement JSONL append
        Ok(())
    }

    /// Compact a session by summarizing older turns.
    pub fn compact(&self, _chat_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        // TODO: implement compaction
        Ok(())
    }

    /// Clear a session.
    pub fn clear(&self, _chat_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        // TODO: implement clear
        Ok(())
    }

    pub fn sessions_dir(&self) -> &str {
        &self.sessions_dir
    }
}
