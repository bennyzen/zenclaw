use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;

use crate::core::types::{Role, ToolCall};

// ---------------------------------------------------------------------------
// Session entry types
// ---------------------------------------------------------------------------

/// A single entry in a session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "message")]
    Message {
        id: String,
        parent: Option<String>,
        role: Role,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
    },
    #[serde(rename = "compaction")]
    Compaction {
        id: String,
        parent: Option<String>,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: usize,
    },
    #[serde(rename = "session_info")]
    Info { leaf_id: String },
}

impl SessionEntry {
    /// Return the entry id (messages and compactions have one, info does not).
    pub fn id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message { id, .. } => Some(id),
            SessionEntry::Compaction { id, .. } => Some(id),
            SessionEntry::Info { .. } => None,
        }
    }

    /// Return the parent pointer, if any.
    pub fn parent(&self) -> Option<&str> {
        match self {
            SessionEntry::Message { parent, .. } => parent.as_deref(),
            SessionEntry::Compaction { parent, .. } => parent.as_deref(),
            SessionEntry::Info { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Volatile session state (in-memory, persisted to sessions.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub turn_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_channel: Option<String>,
}

// ---------------------------------------------------------------------------
// SessionManager
// ---------------------------------------------------------------------------

/// Manages JSONL-based conversation sessions with branching support.
pub struct SessionManager {
    sessions_dir: String,
    states: HashMap<String, SessionState>,
}

fn safe_chat_id(chat_id: &str) -> String {
    chat_id.replace(':', "_").replace('/', "_")
}

impl SessionManager {
    pub fn new(sessions_dir: &str) -> Self {
        // Ensure the sessions directory exists.
        let _ = fs::create_dir_all(sessions_dir);

        // Load volatile state from {parent}/sessions.json
        let states = Self::load_states(sessions_dir);

        Self {
            sessions_dir: sessions_dir.to_string(),
            states,
        }
    }

    pub fn sessions_dir(&self) -> &str {
        &self.sessions_dir
    }

    // -- State persistence ---------------------------------------------------

    fn states_path(sessions_dir: &str) -> String {
        // sessions_dir is e.g. "data/sessions". States go one level up.
        if let Some(parent) = std::path::Path::new(sessions_dir).parent() {
            format!("{}/sessions.json", parent.display())
        } else {
            format!("{}/sessions.json", sessions_dir)
        }
    }

    fn load_states(sessions_dir: &str) -> HashMap<String, SessionState> {
        let path = Self::states_path(sessions_dir);
        match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn save_states(&self) {
        let path = Self::states_path(&self.sessions_dir);
        if let Ok(data) = serde_json::to_string_pretty(&self.states) {
            let _ = fs::write(&path, data);
        }
    }

    pub fn get_state(&self, chat_id: &str) -> SessionState {
        self.states.get(chat_id).cloned().unwrap_or_default()
    }

    pub fn set_state(&mut self, chat_id: &str, state: SessionState) {
        self.states.insert(chat_id.to_string(), state);
        self.save_states();
    }

    pub fn update_state<F: FnOnce(&mut SessionState)>(&mut self, chat_id: &str, f: F) {
        let state = self.states.entry(chat_id.to_string()).or_default();
        f(state);
        self.save_states();
    }

    // -- File path -----------------------------------------------------------

    fn session_path(&self, chat_id: &str) -> String {
        format!("{}/{}.jsonl", self.sessions_dir, safe_chat_id(chat_id))
    }

    // -- Core operations -----------------------------------------------------

    /// Load all entries from a session JSONL file.
    pub fn load(&self, chat_id: &str) -> Result<Vec<SessionEntry>, Box<dyn std::error::Error>> {
        let path = self.session_path(chat_id);
        let data = match fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut entries = Vec::new();
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEntry>(line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!("Skipping malformed session line: {}", e);
                }
            }
        }
        Ok(entries)
    }

    /// Append an entry to the session file (write-through).
    /// If the entry is a Message or Compaction, also appends a session_info
    /// with the new leaf_id.
    pub fn append(
        &self,
        chat_id: &str,
        entry: &SessionEntry,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = self.session_path(chat_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        let line = serde_json::to_string(entry)?;
        writeln!(file, "{}", line)?;

        // Auto-append session_info when we add a message or compaction
        if let Some(id) = entry.id() {
            let info = SessionEntry::Info {
                leaf_id: id.to_string(),
            };
            let info_line = serde_json::to_string(&info)?;
            writeln!(file, "{}", info_line)?;
        }

        file.flush()?;
        Ok(())
    }

    /// Get the leaf_id for a session (last session_info entry).
    pub fn get_leaf_id(
        &self,
        chat_id: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let entries = self.load(chat_id)?;
        let leaf = entries.iter().rev().find_map(|e| match e {
            SessionEntry::Info { leaf_id } => Some(leaf_id.clone()),
            _ => None,
        });
        Ok(leaf)
    }

    /// Get the current branch: walk parent pointers from leaf to root,
    /// return entries in chronological order (root first).
    pub fn get_branch(
        &self,
        chat_id: &str,
    ) -> Result<Vec<SessionEntry>, Box<dyn std::error::Error>> {
        let entries = self.load(chat_id)?;
        let leaf_id = match entries.iter().rev().find_map(|e| match e {
            SessionEntry::Info { leaf_id } => Some(leaf_id.clone()),
            _ => None,
        }) {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        // Build id -> entry index map (only messages and compactions have ids)
        let mut id_map: HashMap<String, usize> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(id) = entry.id() {
                id_map.insert(id.to_string(), i);
            }
        }

        // Walk from leaf to root
        let mut branch_indices = Vec::new();
        let mut current_id = Some(leaf_id);
        while let Some(cid) = current_id {
            if let Some(&idx) = id_map.get(&cid) {
                branch_indices.push(idx);
                current_id = entries[idx].parent().map(|s| s.to_string());
            } else {
                break;
            }
        }

        // Reverse to get chronological order
        branch_indices.reverse();
        let branch: Vec<SessionEntry> = branch_indices
            .into_iter()
            .map(|i| entries[i].clone())
            .collect();

        Ok(branch)
    }

    /// Clear a session (delete the file).
    pub fn clear(&self, chat_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let path = self.session_path(chat_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// List all session chat_ids (derived from .jsonl filenames).
    pub fn list(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut ids = Vec::new();
        let dir = match fs::read_dir(&self.sessions_dir) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(e) => return Err(e.into()),
        };
        for entry in dir {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".jsonl") {
                ids.push(name.trim_end_matches(".jsonl").to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Compact: keep the most recent `keep_recent` messages, replace all
    /// earlier entries with a single Compaction entry containing the summary.
    /// Rewrites the entire file.
    pub fn compact(
        &self,
        chat_id: &str,
        summary: &str,
        keep_recent: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let branch = self.get_branch(chat_id)?;
        if branch.is_empty() {
            return Ok(());
        }

        // Split: entries to summarize vs entries to keep
        let total = branch.len();
        let split = if total > keep_recent {
            total - keep_recent
        } else {
            0
        };

        if split == 0 {
            // Nothing to compact
            return Ok(());
        }

        let kept = &branch[split..];
        let first_kept_id = kept
            .first()
            .and_then(|e| e.id())
            .unwrap_or("")
            .to_string();

        // Build compaction entry
        let compaction_id = uuid::Uuid::new_v4().to_string();
        let compaction = SessionEntry::Compaction {
            id: compaction_id.clone(),
            parent: None,
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_id,
            tokens_before: split,
        };

        // Rewrite file: compaction + session_info, then kept entries + session_info each
        let path = self.session_path(chat_id);
        let mut file = fs::File::create(&path)?;

        // Write compaction
        writeln!(file, "{}", serde_json::to_string(&compaction)?)?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&SessionEntry::Info {
                leaf_id: compaction_id.clone()
            })?
        )?;

        // Write kept entries, updating the first one's parent to point at compaction
        let mut last_id = compaction_id;
        for (i, entry) in kept.iter().enumerate() {
            let rewritten = match entry {
                SessionEntry::Message {
                    id,
                    parent: _,
                    role,
                    content,
                    tool_calls,
                    tool_call_id,
                } => SessionEntry::Message {
                    id: id.clone(),
                    parent: if i == 0 {
                        Some(last_id.clone())
                    } else {
                        entry.parent().map(|s| s.to_string())
                    },
                    role: role.clone(),
                    content: content.clone(),
                    tool_calls: tool_calls.clone(),
                    tool_call_id: tool_call_id.clone(),
                },
                other => other.clone(),
            };
            writeln!(file, "{}", serde_json::to_string(&rewritten)?)?;
            if let Some(id) = rewritten.id() {
                last_id = id.to_string();
            }
            writeln!(
                file,
                "{}",
                serde_json::to_string(&SessionEntry::Info {
                    leaf_id: last_id.clone()
                })?
            )?;
        }

        file.flush()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Role;
    use std::fs;

    fn temp_dir() -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = format!("/tmp/zenclaw_test_{}", id);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_message(id: &str, parent: Option<&str>, role: Role, content: &str) -> SessionEntry {
        SessionEntry::Message {
            id: id.to_string(),
            parent: parent.map(|s| s.to_string()),
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn test_safe_chat_id() {
        assert_eq!(safe_chat_id("simple"), "simple");
        assert_eq!(safe_chat_id("user:123"), "user_123");
        assert_eq!(safe_chat_id("a/b:c"), "a_b_c");
    }

    #[test]
    fn test_create_append_load() {
        let dir = temp_dir();
        let mgr = SessionManager::new(&dir);

        // New session starts empty
        let entries = mgr.load("test1").unwrap();
        assert!(entries.is_empty());

        // Append a user message
        let msg = make_message("m1", None, Role::User, "hello");
        mgr.append("test1", &msg).unwrap();

        // Load it back
        let entries = mgr.load("test1").unwrap();
        // Should have the message + session_info
        assert_eq!(entries.len(), 2);
        match &entries[0] {
            SessionEntry::Message { id, content, .. } => {
                assert_eq!(id, "m1");
                assert_eq!(content, "hello");
            }
            _ => panic!("Expected Message"),
        }
        match &entries[1] {
            SessionEntry::Info { leaf_id } => assert_eq!(leaf_id, "m1"),
            _ => panic!("Expected Info"),
        }

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_branch_navigation() {
        let dir = temp_dir();
        let mgr = SessionManager::new(&dir);

        // Append 3 linked messages
        let m1 = make_message("m1", None, Role::User, "first");
        let m2 = make_message("m2", Some("m1"), Role::Assistant, "second");
        let m3 = make_message("m3", Some("m2"), Role::User, "third");

        mgr.append("branch_test", &m1).unwrap();
        mgr.append("branch_test", &m2).unwrap();
        mgr.append("branch_test", &m3).unwrap();

        // Get branch from leaf
        let branch = mgr.get_branch("branch_test").unwrap();
        assert_eq!(branch.len(), 3);

        // Verify chronological order
        let ids: Vec<&str> = branch.iter().filter_map(|e| e.id()).collect();
        assert_eq!(ids, vec!["m1", "m2", "m3"]);

        // Leaf id should be m3
        let leaf = mgr.get_leaf_id("branch_test").unwrap();
        assert_eq!(leaf, Some("m3".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compact() {
        let dir = temp_dir();
        let mgr = SessionManager::new(&dir);

        // Append 5 messages
        let m1 = make_message("m1", None, Role::User, "one");
        let m2 = make_message("m2", Some("m1"), Role::Assistant, "two");
        let m3 = make_message("m3", Some("m2"), Role::User, "three");
        let m4 = make_message("m4", Some("m3"), Role::Assistant, "four");
        let m5 = make_message("m5", Some("m4"), Role::User, "five");

        mgr.append("compact_test", &m1).unwrap();
        mgr.append("compact_test", &m2).unwrap();
        mgr.append("compact_test", &m3).unwrap();
        mgr.append("compact_test", &m4).unwrap();
        mgr.append("compact_test", &m5).unwrap();

        // Compact keeping 2 recent
        mgr.compact("compact_test", "Summary of first 3 messages", 2)
            .unwrap();

        // Load entries: should have compaction + info + 2 messages (each with info)
        let entries = mgr.load("compact_test").unwrap();

        // Count entry types
        let compactions: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e, SessionEntry::Compaction { .. }))
            .collect();
        let messages: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e, SessionEntry::Message { .. }))
            .collect();
        assert_eq!(compactions.len(), 1);
        assert_eq!(messages.len(), 2);

        // Verify compaction summary
        match &compactions[0] {
            SessionEntry::Compaction {
                summary,
                tokens_before,
                ..
            } => {
                assert_eq!(summary, "Summary of first 3 messages");
                assert_eq!(*tokens_before, 3);
            }
            _ => unreachable!(),
        }

        // Verify kept messages are m4, m5
        let msg_ids: Vec<&str> = messages.iter().filter_map(|e| e.id()).collect();
        assert_eq!(msg_ids, vec!["m4", "m5"]);

        // Branch should still work: compaction -> m4 -> m5
        let branch = mgr.get_branch("compact_test").unwrap();
        assert_eq!(branch.len(), 3); // compaction + 2 messages
        let branch_ids: Vec<&str> = branch.iter().filter_map(|e| e.id()).collect();
        assert_eq!(branch_ids.len(), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clear_and_list() {
        let dir = temp_dir();
        let mgr = SessionManager::new(&dir);

        // Create two sessions
        let m1 = make_message("m1", None, Role::User, "hello");
        mgr.append("chat_a", &m1).unwrap();
        mgr.append("chat_b", &m1).unwrap();

        let mut list = mgr.list().unwrap();
        list.sort();
        assert_eq!(list, vec!["chat_a", "chat_b"]);

        // Clear one
        mgr.clear("chat_a").unwrap();
        let list = mgr.list().unwrap();
        assert_eq!(list, vec!["chat_b"]);

        // Clear non-existent is fine
        mgr.clear("nonexistent").unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_session_state() {
        let dir = temp_dir();
        // Create parent dir for sessions.json
        let parent = std::path::Path::new(&dir).parent().unwrap();
        let _ = fs::create_dir_all(parent);

        let mut mgr = SessionManager::new(&dir);

        // Default state
        let state = mgr.get_state("chat1");
        assert_eq!(state.turn_count, 0);
        assert!(state.model_override.is_none());

        // Update state
        mgr.update_state("chat1", |s| {
            s.turn_count = 5;
            s.last_channel = Some("telegram".to_string());
        });

        // Read it back
        let state = mgr.get_state("chat1");
        assert_eq!(state.turn_count, 5);
        assert_eq!(state.last_channel.as_deref(), Some("telegram"));

        // Persistence: create a new manager from same dir
        let mgr2 = SessionManager::new(&dir);
        let state = mgr2.get_state("chat1");
        assert_eq!(state.turn_count, 5);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_empty_branch() {
        let dir = temp_dir();
        let mgr = SessionManager::new(&dir);

        let branch = mgr.get_branch("nonexistent").unwrap();
        assert!(branch.is_empty());

        let leaf = mgr.get_leaf_id("nonexistent").unwrap();
        assert!(leaf.is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
