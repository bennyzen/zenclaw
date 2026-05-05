use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::sync::Arc;

use crate::core::cloud::{CloudCache, Replicator};
use crate::core::types::{Role, ToolCall};

pub mod meta;

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
    /// Cloud Tier-1 cache. `None` → local-file path. `Some` → all reads
    /// come from here, all writes update here + enqueue to `replicator`.
    cache: Option<CloudCache>,
    /// Eager-path replicator. Always set together with `cache`.
    replicator: Option<Arc<Replicator>>,
    /// Threshold (bytes) at which the active log file gets folded into
    /// `base.jsonl` and a new log file starts. Mirrors
    /// [`crate::config::StorageConfig::log_compaction_bytes`].
    log_compaction_bytes: usize,
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
            cache: None,
            replicator: None,
            log_compaction_bytes: 0,
        }
    }

    /// Enable cloud persistence. After this, `append` writes through the
    /// cache + replicator instead of the local filesystem, and `load`
    /// reads from `base.jsonl` + unabsorbed `log-NN.jsonl` in the cache.
    pub fn with_cloud(
        mut self,
        cache: CloudCache,
        replicator: Arc<Replicator>,
        log_compaction_bytes: usize,
    ) -> Self {
        self.cache = Some(cache);
        self.replicator = Some(replicator);
        self.log_compaction_bytes = log_compaction_bytes;
        self
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

    /// Path to the per-session metadata sidecar (`<chat_id>.meta.json`)
    /// on local disk. Mirrors `session_path`'s sanitization rules.
    fn meta_path(&self, chat_id: &str) -> String {
        format!("{}/{}.meta.json", self.sessions_dir, safe_chat_id(chat_id))
    }

    /// Read this session's metadata sidecar from local disk. `None` if
    /// no sidecar exists; the caller may synthesize a default via
    /// `SessionMeta::synthesize_default`. Cloud lookups happen at boot
    /// (cloud/boot.rs), not here — this is the hot path for the
    /// sidebar list.
    pub fn meta(
        &self,
        chat_id: &str,
    ) -> Result<Option<crate::core::sessions::meta::SessionMeta>, Box<dyn std::error::Error>> {
        let path = self.meta_path(chat_id);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// On-disk byte size of the session JSONL, or None if it doesn't exist.
    /// In cloud mode, returns the sum of `base.jsonl` plus all unabsorbed
    /// `log-NN.jsonl` bytes for this chat — i.e. the materialized view.
    pub fn session_size_bytes(&self, chat_id: &str) -> Option<usize> {
        if let Some(cache) = &self.cache {
            return Some(self.cloud_session_bytes(chat_id, cache));
        }
        let path = self.session_path(chat_id);
        fs::metadata(&path).ok().map(|m| m.len() as usize)
    }

    // -- Core operations -----------------------------------------------------

    /// Load all entries from a session JSONL file.
    pub fn load(&self, chat_id: &str) -> Result<Vec<SessionEntry>, Box<dyn std::error::Error>> {
        let data = if let Some(cache) = &self.cache {
            self.cloud_load_text(chat_id, cache)
        } else {
            let path = self.session_path(chat_id);
            match fs::read_to_string(&path) {
                Ok(d) => d,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(e.into()),
            }
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

    // -- Cloud helpers (active when `cache` is Some) -------------------------

    /// `sys/sessions/{chat_id}/...` key prefix for this chat (uses the
    /// same `safe_chat_id` sanitization as the local-file path).
    fn cloud_prefix(chat_id: &str) -> String {
        format!("sys/sessions/{}/", safe_chat_id(chat_id))
    }

    fn cloud_base_key(chat_id: &str) -> String {
        format!("{}base.jsonl", Self::cloud_prefix(chat_id))
    }

    fn cloud_meta_key(chat_id: &str) -> String {
        format!("{}base.meta.json", Self::cloud_prefix(chat_id))
    }

    fn cloud_log_key(chat_id: &str, idx: u32) -> String {
        format!("{}log-{:02}.jsonl", Self::cloud_prefix(chat_id), idx)
    }

    /// Highest log index that has been folded into base.jsonl, per
    /// `base.meta.json`. None if no compaction has happened yet.
    fn cloud_highest_absorbed(&self, chat_id: &str, cache: &CloudCache) -> Option<u32> {
        cache
            .get(&Self::cloud_meta_key(chat_id))
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| {
                v.get("highest_absorbed_log")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as u32)
            })
    }

    /// Index of the active (writable) log file. After absorbing log-N
    /// into base, future appends go to log-(N+1).
    fn current_log_index(&self, chat_id: &str, cache: &CloudCache) -> u32 {
        self.cloud_highest_absorbed(chat_id, cache)
            .map(|n| n + 1)
            .unwrap_or(0)
    }

    /// Concat base.jsonl + all unabsorbed log-NN.jsonl in numeric order.
    fn cloud_load_text(&self, chat_id: &str, cache: &CloudCache) -> String {
        let mut out = String::new();
        if let Some(base) = cache.get(&Self::cloud_base_key(chat_id)) {
            out.push_str(&String::from_utf8_lossy(&base));
        }
        let absorbed = self.cloud_highest_absorbed(chat_id, cache);
        let prefix = format!("{}log-", Self::cloud_prefix(chat_id));
        let mut indices: Vec<u32> = cache
            .keys_with_prefix(&prefix)
            .iter()
            .filter_map(|k| {
                k.strip_prefix(&prefix)?
                    .strip_suffix(".jsonl")?
                    .parse::<u32>()
                    .ok()
            })
            .filter(|n| absorbed.map(|a| *n > a).unwrap_or(true))
            .collect();
        indices.sort();
        for idx in indices {
            if let Some(log) = cache.get(&Self::cloud_log_key(chat_id, idx)) {
                out.push_str(&String::from_utf8_lossy(&log));
            }
        }
        out
    }

    /// Sum of base.jsonl + unabsorbed logs in the cache. Used by
    /// `session_size_bytes` so callers (e.g. compaction trigger in
    /// `gateway`) get the materialized view, not the on-disk file size.
    fn cloud_session_bytes(&self, chat_id: &str, cache: &CloudCache) -> usize {
        let base_len = cache
            .get(&Self::cloud_base_key(chat_id))
            .map(|v| v.len())
            .unwrap_or(0);
        let absorbed = self.cloud_highest_absorbed(chat_id, cache);
        let prefix = format!("{}log-", Self::cloud_prefix(chat_id));
        let logs_len: usize = cache
            .keys_with_prefix(&prefix)
            .iter()
            .filter_map(|k| {
                let idx = k
                    .strip_prefix(&prefix)?
                    .strip_suffix(".jsonl")?
                    .parse::<u32>()
                    .ok()?;
                if absorbed.map(|a| idx > a).unwrap_or(true) {
                    cache.get(k).map(|v| v.len())
                } else {
                    None
                }
            })
            .sum();
        base_len + logs_len
    }

    /// Append `lines` (already serialized + newline-terminated) to the
    /// active log in the cache, enqueue the resulting log blob to the
    /// replicator, and run compaction if the active log crossed the
    /// `log_compaction_bytes` threshold.
    fn append_via_cloud(
        &self,
        chat_id: &str,
        lines: &[u8],
        cache: &CloudCache,
        replicator: &Arc<Replicator>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let log_idx = self.current_log_index(chat_id, cache);
        let log_key = Self::cloud_log_key(chat_id, log_idx);

        let mut current = cache.get(&log_key).unwrap_or_default();
        current.extend_from_slice(lines);
        cache.put(&log_key, current.clone());
        replicator.enqueue(log_key.clone(), current.clone());

        if current.len() >= self.log_compaction_bytes {
            self.compact_log(chat_id, log_idx, cache, replicator)?;
        }
        Ok(())
    }

    /// Fold the active log into `base.jsonl` and bump `highest_absorbed_log`.
    /// The next call to `current_log_index` will return `log_idx + 1`, so
    /// future appends go to a fresh log file. The absorbed log key stays in
    /// the cache (cheap to keep; matters for boot-time crash recovery).
    fn compact_log(
        &self,
        chat_id: &str,
        log_idx: u32,
        cache: &CloudCache,
        replicator: &Arc<Replicator>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base_key = Self::cloud_base_key(chat_id);
        let log_key = Self::cloud_log_key(chat_id, log_idx);
        let meta_key = Self::cloud_meta_key(chat_id);

        let mut new_base = cache.get(&base_key).unwrap_or_default();
        if let Some(log) = cache.get(&log_key) {
            new_base.extend_from_slice(&log);
        }

        cache.put(&base_key, new_base.clone());
        replicator.enqueue(base_key, new_base);

        let meta = serde_json::json!({ "highest_absorbed_log": log_idx }).to_string();
        let meta_bytes = meta.into_bytes();
        cache.put(&meta_key, meta_bytes.clone());
        replicator.enqueue(meta_key, meta_bytes);

        Ok(())
    }

    /// Append an entry to the session file (write-through).
    /// If the entry is a Message or Compaction with `parent: None`, the
    /// parent is auto-linked to the current leaf_id of the session — this
    /// keeps `get_branch` traversable as a single chronological chain.
    /// Callers that genuinely want a detached entry (none currently exist)
    /// would need a separate API.
    /// If the entry is a Message or Compaction, also appends a session_info
    /// with the new leaf_id.
    pub fn append(
        &self,
        chat_id: &str,
        entry: &SessionEntry,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Auto-link parent for Message/Compaction entries that were
        // constructed with `parent: None`. We compute it from the current
        // leaf_id (cache-aware via load() → cloud-aware fork); if there is
        // no prior leaf this remains None (i.e. the new entry is the root
        // of a fresh session).
        let to_write = match entry {
            SessionEntry::Message {
                id,
                parent: None,
                role,
                content,
                tool_calls,
                tool_call_id,
            } => {
                let parent = self.get_leaf_id(chat_id).ok().flatten();
                SessionEntry::Message {
                    id: id.clone(),
                    parent,
                    role: role.clone(),
                    content: content.clone(),
                    tool_calls: tool_calls.clone(),
                    tool_call_id: tool_call_id.clone(),
                }
            }
            SessionEntry::Compaction {
                id,
                parent: None,
                summary,
                first_kept_entry_id,
                tokens_before,
            } => {
                let parent = self.get_leaf_id(chat_id).ok().flatten();
                SessionEntry::Compaction {
                    id: id.clone(),
                    parent,
                    summary: summary.clone(),
                    first_kept_entry_id: first_kept_entry_id.clone(),
                    tokens_before: *tokens_before,
                }
            }
            other => other.clone(),
        };

        // Build the bytes we'd write — entry line plus an optional
        // session_info line (mirrors the historic local-file layout so
        // load() can use the same parser for both paths).
        let mut bytes = Vec::with_capacity(256);
        bytes.extend_from_slice(serde_json::to_string(&to_write)?.as_bytes());
        bytes.push(b'\n');
        if let Some(id) = to_write.id() {
            let info = SessionEntry::Info {
                leaf_id: id.to_string(),
            };
            bytes.extend_from_slice(serde_json::to_string(&info)?.as_bytes());
            bytes.push(b'\n');
        }

        if let (Some(cache), Some(replicator)) = (&self.cache, &self.replicator) {
            self.append_via_cloud(chat_id, &bytes, cache, replicator)?;
        } else {
            let path = self.session_path(chat_id);
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            file.write_all(&bytes)?;
            file.flush()?;
        }
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

    /// Wipe the session for `chat_id`.
    ///
    /// Local mode: deletes `{sessions_dir}/{chat_id}.jsonl`.
    /// Cloud mode: also drops every cache key matching `sessions/{chat_id}/`
    /// and, when an `ObjectStore` is supplied via `clear_with_store`,
    /// issues a best-effort S3 delete for the listed keys.
    ///
    /// Preserves `SessionState` (model_override, turn_count, last_channel) —
    /// the user's fast-dial setting must survive a clear.
    pub fn clear(&self, chat_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Cloud-side cache wipe (in-memory tier-1).
        if let Some(cache) = &self.cache {
            let prefix = format!("sessions/{}/", safe_chat_id(chat_id));
            for key in cache.keys_with_prefix(&prefix) {
                cache.delete(&key);
            }
        }

        // Local-file mode (or fallback if the cache has no entries).
        let path = self.session_path(chat_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// As `clear`, but also issues best-effort `delete` for matching keys
    /// against `store`. Used by `/clear` so S3-side state is wiped too.
    /// Errors from individual deletes are logged and swallowed —
    /// `/clear` is destructive and idempotent; partial S3 failures are
    /// recoverable by re-running.
    pub fn clear_with_store(
        &self,
        chat_id: &str,
        store: &dyn crate::core::cloud::client::ObjectStore,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let prefix = format!("sessions/{}/", safe_chat_id(chat_id));
        match store.list_keys(&prefix) {
            Ok(keys) => {
                for k in keys {
                    if let Err(e) = store.delete(&k) {
                        tracing::warn!(error = %e, key = %k, "/clear: S3 delete failed");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, prefix = %prefix, "/clear: S3 list_keys failed");
            }
        }
        self.clear(chat_id)
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
    ///
    /// `max_kept_message_bytes`: if Some(cap), any kept Message whose
    /// content exceeds the cap has its content replaced with a short
    /// redaction marker. Stops a single huge tool result inside the
    /// `keep_recent` window from re-tripping the byte threshold on the
    /// next turn — the summary is expected to carry the gist forward.
    pub fn compact(
        &self,
        chat_id: &str,
        summary: &str,
        keep_recent: usize,
        max_kept_message_bytes: Option<usize>,
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
                } => {
                    let kept_content = match max_kept_message_bytes {
                        Some(cap) if content.len() > cap => format!(
                            "[redacted during compaction: {} bytes — see preceding compaction summary]",
                            content.len()
                        ),
                        _ => content.clone(),
                    };
                    SessionEntry::Message {
                        id: id.clone(),
                        parent: if i == 0 {
                            Some(last_id.clone())
                        } else {
                            entry.parent().map(|s| s.to_string())
                        },
                        role: role.clone(),
                        content: kept_content,
                        tool_calls: tool_calls.clone(),
                        tool_call_id: tool_call_id.clone(),
                    }
                }
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
        let dir = format!("/tmp/zenclaw_test_{}/sessions", id);
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
        mgr.compact("compact_test", "Summary of first 3 messages", 2, None)
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

    // -- Cloud routing tests -------------------------------------------------

    fn cloud_mgr(
        log_compaction_bytes: usize,
    ) -> (SessionManager, CloudCache, std::sync::Arc<Replicator>, String) {
        use crate::core::cloud::ReplicatorConfig;
        let dir = temp_dir();
        let cache = CloudCache::new();
        let cfg = ReplicatorConfig {
            queue_max: 32,
            retry_max: 1,
            backoff_cap_secs: 1,
        };
        let replicator = std::sync::Arc::new(Replicator::new(cfg));
        let mgr = SessionManager::new(&dir).with_cloud(
            cache.clone(),
            replicator.clone(),
            log_compaction_bytes,
        );
        (mgr, cache, replicator, dir)
    }

    #[test]
    fn append_when_cloud_enabled_writes_to_log_in_cache_and_replicator() {
        let (mgr, cache, replicator, dir) = cloud_mgr(16_384);

        let entry = make_message("msg1", None, Role::User, "hello");
        mgr.append("web", &entry).unwrap();

        // Cache holds log-00 with the entry + the auto-info line.
        let log_key = "sys/sessions/web/log-00.jsonl";
        let cached = cache.get(log_key).expect("log-00 cached");
        let s = String::from_utf8_lossy(&cached);
        assert!(s.contains("msg1"), "cached log = {}", s);
        assert!(s.contains("session_info"), "auto-info missing: {}", s);

        // Replicator queue picked up at least one PUT.
        assert!(replicator.queue_depth() >= 1);

        // Local file path was NOT touched in cloud mode.
        let local = format!("{}/web.jsonl", dir);
        assert!(!std::path::Path::new(&local).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_load_round_trips_through_cache() {
        let (mgr, _cache, _replicator, dir) = cloud_mgr(16_384);

        mgr.append("web", &make_message("m1", None, Role::User, "first"))
            .unwrap();
        mgr.append("web", &make_message("m2", None, Role::User, "second"))
            .unwrap();

        let entries = mgr.load("web").unwrap();
        // Two messages + two auto-info entries.
        assert_eq!(entries.len(), 4);

        // Auto-link from m2 should point at m1 (via the leaf_id resolved
        // from the cache, not the missing local file).
        let m2 = entries.iter().find(|e| e.id() == Some("m2")).unwrap();
        assert_eq!(m2.parent(), Some("m1"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_compacts_log_when_threshold_exceeded() {
        // Tiny threshold: a single padded message trips compaction.
        let (mgr, cache, _replicator, dir) = cloud_mgr(32);

        for i in 0..3 {
            let entry = make_message(
                &format!("msg{}", i),
                None,
                Role::User,
                &format!("content-of-message-number-{}-padding-padding", i),
            );
            mgr.append("web", &entry).unwrap();
        }

        // base.jsonl populated by compaction.
        let base = cache
            .get("sys/sessions/web/base.jsonl")
            .expect("base.jsonl populated after compaction");
        let base_str = String::from_utf8_lossy(&base);
        for i in 0..3 {
            assert!(
                base_str.contains(&format!("msg{}", i)),
                "msg{} missing from base: {}",
                i,
                base_str
            );
        }

        // Meta tracks highest_absorbed_log.
        let meta = cache.get("sys/sessions/web/base.meta.json").unwrap();
        let meta_str = String::from_utf8_lossy(&meta);
        assert!(meta_str.contains("highest_absorbed_log"));

        // load() reads base only (logs absorbed) and returns a coherent
        // chronological stream — every msgN should appear once.
        let entries = mgr.load("web").unwrap();
        let ids: Vec<&str> = entries.iter().filter_map(|e| e.id()).collect();
        assert_eq!(ids, vec!["msg0", "msg1", "msg2"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_when_cloud_disabled_uses_local_file_only() {
        let dir = temp_dir();
        let mgr = SessionManager::new(&dir);

        mgr.append("web", &make_message("m", None, Role::User, "hi"))
            .unwrap();

        let path = format!("{}/web.jsonl", dir);
        assert!(std::path::Path::new(&path).exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("hi"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_in_cloud_mode_drops_cache_keys_for_chat_id() {
        use crate::core::cloud::{CloudCache, Replicator};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let cache = CloudCache::new();
        cache.put(
            "sessions/abc/base.jsonl",
            b"{\"role\":\"user\"}\n".to_vec(),
        );
        cache.put(
            "sessions/abc/log-00.jsonl",
            b"{\"role\":\"assistant\"}\n".to_vec(),
        );
        cache.put(
            "sessions/other/base.jsonl",
            b"different chat".to_vec(),
        );
        // Replicator with a no-op store — we only verify cache wipe here.
        let replicator = Arc::new(Replicator::new_for_test());

        let mgr = SessionManager::new(dir.path().to_str().unwrap())
            .with_cloud(cache.clone(), replicator, 0);

        mgr.clear("abc").unwrap();

        // Both `abc` keys gone, `other` untouched.
        assert!(cache.get("sessions/abc/base.jsonl").is_none());
        assert!(cache.get("sessions/abc/log-00.jsonl").is_none());
        assert!(cache.get("sessions/other/base.jsonl").is_some());
    }

    #[test]
    fn clear_local_mode_still_deletes_file() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let path = sessions_dir.join("abc.jsonl");
        std::fs::write(&path, "{}\n").unwrap();

        let mgr = SessionManager::new(sessions_dir.to_str().unwrap());
        mgr.clear("abc").unwrap();

        assert!(!path.exists(), "session file should be deleted");
    }

    #[test]
    fn meta_returns_none_when_sidecar_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        assert!(mgr.meta("nonexistent").unwrap().is_none());
    }

    #[test]
    fn meta_reads_existing_sidecar() {
        use crate::core::sessions::meta::{SessionKind, SessionMeta, TitleSource};
        let dir = tempfile::tempdir().unwrap();
        let m = SessionMeta {
            chat_id: "chat-1".into(),
            kind: SessionKind::Web,
            title: "Persisted".into(),
            title_source: TitleSource::User,
            created_at_ms: 1,
            last_activity_ms: 2,
            last_message_preview: "p".into(),
            version: 1,
        };
        let path = dir.path().join("chat-1.meta.json");
        std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();

        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let loaded = mgr.meta("chat-1").unwrap().unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn cloud_session_size_bytes_sums_base_and_unabsorbed_logs() {
        let (mgr, cache, _replicator, dir) = cloud_mgr(16_384);

        // No data → 0.
        assert_eq!(mgr.session_size_bytes("web"), Some(0));

        mgr.append("web", &make_message("m1", None, Role::User, "hello"))
            .unwrap();

        let log_len = cache.get("sys/sessions/web/log-00.jsonl").unwrap().len();
        assert_eq!(mgr.session_size_bytes("web"), Some(log_len));

        let _ = fs::remove_dir_all(&dir);
    }
}
