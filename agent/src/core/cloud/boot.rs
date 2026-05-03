//! Boot-time restore sequence — populates the PSRAM cache from S3 with
//! defense-in-depth safety layers (L3 size gate, L4 tail-only fallback,
//! L5 quarantine). Implements steps 4-8 of spec §6.6 — config restore
//! (step 3) lives in `main.rs`, drainer + agent-loop start (step 9) too.
//!
//! All cache writes are local (no replicator round-trip) — boot is the
//! one moment we *want* PSRAM to mirror S3 byte-for-byte, not enqueue
//! the bytes back as new writes.
//!
//! Failure philosophy: every step is best-effort. The device must boot
//! even when S3 is unreachable, when individual chats are corrupted, or
//! when the heartbeat shows another device claimed the bucket. Warnings
//! surface to `/api/status`; quarantines surface to a UI banner.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;

use crate::core::cloud::cache::CloudCache;
use crate::core::cloud::client::{ObjectStore, S3Error};

#[derive(Debug, Clone, Serialize)]
pub struct BootWarning {
    pub kind: BootWarningKind,
    /// Owning chat_id for L3/L4/L5; empty string for non-chat warnings.
    pub chat_id: String,
    /// Unix timestamp (seconds) — when the warning was generated.
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BootWarningKind {
    /// L4 tripped: only the last `kept_size` bytes of the highest log
    /// were retained. Original session size shown so the UI can explain
    /// "5 KB of 312 KB kept".
    Truncated {
        original_size: u64,
        kept_size: u64,
    },
    /// L5 tripped: the chat's keys were moved to `.quarantine/` and the
    /// session reset to empty.
    Quarantined,
}

#[derive(Debug)]
pub struct BootResult {
    pub warnings: Vec<BootWarning>,
    /// Set when the bucket's heartbeat showed a different, recent
    /// device_id (i.e. another device is also writing here). The
    /// caller surfaces this on the dashboard but boot proceeds.
    pub heartbeat_conflict: Option<String>,
}

#[derive(Clone)]
pub struct BootConfig {
    /// L3 trigger: total HEAD'd bytes for a chat above this triggers L4.
    pub session_max_bytes: usize,
    /// L4 retention budget: tail bytes of the highest log to keep.
    pub log_compaction_bytes: usize,
    /// MAC-derived hex hostname suffix; written to `sys/.heartbeat`.
    pub device_id: String,
    /// Heartbeat staleness window. A heartbeat from a *different* device
    /// is treated as a conflict only if newer than `now - this`.
    pub heartbeat_stale_secs: i64,
}

impl Default for BootConfig {
    fn default() -> Self {
        Self {
            session_max_bytes: 256_000,
            log_compaction_bytes: 16_384,
            device_id: "unknown".to_string(),
            heartbeat_stale_secs: 3600,
        }
    }
}

/// Run the full boot sequence. Returns warnings and any heartbeat
/// conflict. Errors only on truly fatal LIST failures at the top level —
/// per-chat failures are absorbed into warnings.
pub fn boot_restore(
    store: &Arc<dyn ObjectStore>,
    cache: &CloudCache,
    cfg: &BootConfig,
) -> Result<BootResult, S3Error> {
    let mut warnings = Vec::new();

    // Step 4: heartbeat check (then write our own).
    let heartbeat_conflict = check_heartbeat(store, cfg);
    let _ = write_heartbeat(store, cache, cfg);

    // Step 5: enumerate chats, restore each.
    let chat_keys = store.list_keys("sys/sessions/")?;
    let chat_ids = unique_chat_ids(&chat_keys);
    for chat_id in chat_ids {
        match restore_chat(store, cache, &chat_id, cfg) {
            Ok(Some(w)) => warnings.push(w),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("boot_restore: chat {} skipped: {}", chat_id, e);
            }
        }
    }

    // Step 6: memory restore (best-effort).
    if let Ok(memory) = store.get("sys/MEMORY.md") {
        cache.put("sys/MEMORY.md", memory);
    }
    // Step 7: cron restore (best-effort).
    if let Ok(cron) = store.get("sys/cron.json") {
        cache.put("sys/cron.json", cron);
    }
    // Step 8: identity files (best-effort, optional).
    for key in ["sys/SOUL.md", "sys/AGENTS.md"] {
        if let Ok(bytes) = store.get(key) {
            cache.put(key, bytes);
        }
    }

    Ok(BootResult {
        warnings,
        heartbeat_conflict,
    })
}

fn now_unix() -> i64 {
    Utc::now().timestamp()
}

/// Extract the unique `{chat_id}` segments from a list of
/// `sys/sessions/{chat_id}/...` keys. Skips `.quarantine/` and any
/// entries that don't fit the expected shape.
fn unique_chat_ids(keys: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for k in keys {
        if let Some(rest) = k.strip_prefix("sys/sessions/") {
            if let Some((chat_id, _)) = rest.split_once('/') {
                if !chat_id.is_empty() && !chat_id.starts_with('.') {
                    seen.insert(chat_id.to_string());
                }
            }
        }
    }
    seen.into_iter().collect()
}

#[derive(serde::Deserialize)]
struct HeartbeatRecord {
    device_id: String,
    ts: i64,
}

fn check_heartbeat(store: &Arc<dyn ObjectStore>, cfg: &BootConfig) -> Option<String> {
    let bytes = store.get("sys/.heartbeat").ok()?;
    let record: HeartbeatRecord = serde_json::from_slice(&bytes).ok()?;
    if record.device_id == cfg.device_id {
        return None; // ours
    }
    let age = now_unix() - record.ts;
    if age >= 0 && age < cfg.heartbeat_stale_secs {
        Some(record.device_id) // recent + foreign → conflict
    } else {
        None // stale; we'll overwrite
    }
}

fn write_heartbeat(
    store: &Arc<dyn ObjectStore>,
    cache: &CloudCache,
    cfg: &BootConfig,
) -> Result<(), S3Error> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "device_id": cfg.device_id,
        "ts": now_unix(),
    }))
    .map_err(|e| S3Error(format!("heartbeat serialize: {}", e)))?;
    cache.put("sys/.heartbeat", bytes.clone());
    store.put("sys/.heartbeat", &bytes)
}

/// Restore a single chat's session keys into the cache, applying L3/L4/L5
/// as needed. Returns Some(warning) if a safety layer fired.
fn restore_chat(
    store: &Arc<dyn ObjectStore>,
    cache: &CloudCache,
    chat_id: &str,
    cfg: &BootConfig,
) -> Result<Option<BootWarning>, S3Error> {
    let prefix = format!("sys/sessions/{}/", chat_id);
    let quarantine_prefix = format!("{}.quarantine/", prefix);

    let all_keys = store.list_keys(&prefix)?;
    let session_keys: Vec<String> = all_keys
        .into_iter()
        .filter(|k| !k.starts_with(&quarantine_prefix))
        .collect();
    if session_keys.is_empty() {
        return Ok(None);
    }

    // L3: HEAD each, sum sizes.
    let mut sizes: Vec<(String, u64)> = Vec::with_capacity(session_keys.len());
    let mut total: u64 = 0;
    for key in &session_keys {
        let len = store.head(key)?.unwrap_or(0);
        sizes.push((key.clone(), len));
        total += len;
    }

    if total <= cfg.session_max_bytes as u64 {
        // Within budget — full restore.
        for (key, _) in &sizes {
            match store.get(key) {
                Ok(bytes) => cache.put(key, bytes),
                Err(_) => {
                    // GET succeeded HEAD but not body — escalate to L4.
                    return Ok(Some(l4_or_l5(store, cache, chat_id, &sizes, cfg)?));
                }
            }
        }
        Ok(None)
    } else {
        // L3 trips → L4.
        Ok(Some(l4_or_l5(store, cache, chat_id, &sizes, cfg)?))
    }
}

/// Tail-fallback (L4); if that also fails, quarantine (L5).
fn l4_or_l5(
    store: &Arc<dyn ObjectStore>,
    cache: &CloudCache,
    chat_id: &str,
    sizes: &[(String, u64)],
    cfg: &BootConfig,
) -> Result<BootWarning, S3Error> {
    let log_prefix = format!("sys/sessions/{}/log-", chat_id);
    let highest_log = sizes
        .iter()
        .filter_map(|(k, len)| {
            let n = k
                .strip_prefix(&log_prefix)?
                .strip_suffix(".jsonl")?
                .parse::<u32>()
                .ok()?;
            Some((n, k.clone(), *len))
        })
        .max_by_key(|(n, _, _)| *n);

    let Some((_idx, log_key, total_len)) = highest_log else {
        return l5_quarantine(store, cache, chat_id, sizes);
    };

    let tail_size = (cfg.log_compaction_bytes as u64).min(total_len);
    if tail_size == 0 {
        return l5_quarantine(store, cache, chat_id, sizes);
    }
    let offset = total_len.saturating_sub(tail_size);

    let bytes = match store.get_range(&log_key, offset, tail_size) {
        Ok(b) => b,
        Err(_) => return l5_quarantine(store, cache, chat_id, sizes),
    };

    // Drop everything before the first newline (might be a partial JSON
    // line from when we sliced into the middle of an entry).
    let kept: &[u8] = match bytes.iter().position(|&b| b == b'\n') {
        Some(p) => &bytes[p + 1..],
        None => &bytes[..],
    };
    if !validate_jsonl(kept) {
        return l5_quarantine(store, cache, chat_id, sizes);
    }

    cache.put(&log_key, kept.to_vec());
    Ok(BootWarning {
        kind: BootWarningKind::Truncated {
            original_size: total_len,
            kept_size: kept.len() as u64,
        },
        chat_id: chat_id.to_string(),
        at: now_unix(),
    })
}

/// L5: best-effort move every key to `.quarantine/`, then delete the
/// originals. We *try* to copy first so the user can recover offline,
/// but if get-then-put fails we still delete — the alternative is
/// cycling through L3→L4→L5 forever on every reboot.
fn l5_quarantine(
    store: &Arc<dyn ObjectStore>,
    _cache: &CloudCache,
    chat_id: &str,
    sizes: &[(String, u64)],
) -> Result<BootWarning, S3Error> {
    let prefix = format!("sys/sessions/{}/", chat_id);
    let qprefix = format!("{}.quarantine/", prefix);
    for (key, _) in sizes {
        let suffix = key.strip_prefix(&prefix).unwrap_or(key);
        let qkey = format!("{}{}", qprefix, suffix);
        if let Ok(bytes) = store.get(key) {
            let _ = store.put(&qkey, &bytes);
        }
        let _ = store.delete(key);
    }
    Ok(BootWarning {
        kind: BootWarningKind::Quarantined,
        chat_id: chat_id.to_string(),
        at: now_unix(),
    })
}

/// Cheap JSONL validation — every non-empty line must be valid JSON.
/// We don't validate the schema (SessionEntry::deserialize) because L5
/// must remain a fast pre-flight check even on tens of KB of bytes.
fn validate_jsonl(bytes: &[u8]) -> bool {
    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if serde_json::from_str::<serde_json::Value>(line).is_err() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cloud::client::Result as S3Result;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// FakeStore tailored for boot tests — supports list_keys, head,
    /// get, get_range, put, delete. Tracks all puts + deletes for
    /// assertions about quarantine behavior.
    struct FakeStore {
        objects: Mutex<HashMap<String, Vec<u8>>>,
        puts: Mutex<Vec<(String, Vec<u8>)>>,
        deletes: Mutex<Vec<String>>,
        get_fails: Mutex<Vec<String>>,        // keys to fail GET on
        get_range_fails: Mutex<Vec<String>>,  // keys to fail GET-range on
    }
    impl FakeStore {
        fn new() -> Self {
            Self {
                objects: Mutex::new(HashMap::new()),
                puts: Mutex::new(vec![]),
                deletes: Mutex::new(vec![]),
                get_fails: Mutex::new(vec![]),
                get_range_fails: Mutex::new(vec![]),
            }
        }
        fn seed(&self, key: &str, bytes: Vec<u8>) {
            self.objects.lock().unwrap().insert(key.to_string(), bytes);
        }
        fn fail_get(&self, key: &str) {
            self.get_fails.lock().unwrap().push(key.to_string());
        }
        fn fail_get_range(&self, key: &str) {
            self.get_range_fails.lock().unwrap().push(key.to_string());
        }
        fn deletes(&self) -> Vec<String> {
            self.deletes.lock().unwrap().clone()
        }
        fn puts(&self) -> Vec<(String, Vec<u8>)> {
            self.puts.lock().unwrap().clone()
        }
    }
    impl ObjectStore for FakeStore {
        fn put(&self, key: &str, bytes: &[u8]) -> S3Result<()> {
            self.puts
                .lock()
                .unwrap()
                .push((key.to_string(), bytes.to_vec()));
            self.objects
                .lock()
                .unwrap()
                .insert(key.to_string(), bytes.to_vec());
            Ok(())
        }
        fn get(&self, key: &str) -> S3Result<Vec<u8>> {
            if self.get_fails.lock().unwrap().iter().any(|k| k == key) {
                return Err(S3Error("forced GET fail".to_string()));
            }
            match self.objects.lock().unwrap().get(key) {
                Some(b) => Ok(b.clone()),
                None => Err(S3Error("not found".to_string())),
            }
        }
        fn delete(&self, key: &str) -> S3Result<()> {
            self.deletes.lock().unwrap().push(key.to_string());
            self.objects.lock().unwrap().remove(key);
            Ok(())
        }
        fn head(&self, key: &str) -> S3Result<Option<u64>> {
            Ok(self
                .objects
                .lock()
                .unwrap()
                .get(key)
                .map(|b| b.len() as u64))
        }
        fn list_keys(&self, prefix: &str) -> S3Result<Vec<String>> {
            Ok(self
                .objects
                .lock()
                .unwrap()
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect())
        }
        fn get_range(&self, key: &str, offset: u64, length: u64) -> S3Result<Vec<u8>> {
            if self.get_range_fails.lock().unwrap().iter().any(|k| k == key) {
                return Err(S3Error("forced GET-range fail".to_string()));
            }
            let g = self.objects.lock().unwrap();
            let bytes = g
                .get(key)
                .ok_or_else(|| S3Error("not found".to_string()))?;
            let off = offset as usize;
            let len = length as usize;
            let end = (off + len).min(bytes.len());
            Ok(bytes[off.min(bytes.len())..end].to_vec())
        }
    }

    fn cfg() -> BootConfig {
        BootConfig {
            session_max_bytes: 1024,
            log_compaction_bytes: 64,
            device_id: "device-aaaaaa".to_string(),
            heartbeat_stale_secs: 3600,
        }
    }

    fn jsonl_line(id: &str) -> String {
        format!(
            r#"{{"type":"message","id":"{}","role":"user","content":"x"}}"#,
            id
        )
    }

    #[test]
    fn within_budget_loads_full_session() {
        let fake = FakeStore::new();
        // base + log-00, both small → total well under session_max_bytes.
        let base = format!("{}\n{}\n", jsonl_line("m1"), jsonl_line("m2"));
        let log0 = format!("{}\n", jsonl_line("m3"));
        fake.seed("sys/sessions/web/base.jsonl", base.into_bytes());
        fake.seed("sys/sessions/web/log-00.jsonl", log0.into_bytes());

        let store: Arc<dyn ObjectStore> = Arc::new(fake);
        let cache = CloudCache::new();

        let result = boot_restore(&store, &cache, &cfg()).unwrap();
        assert!(result.warnings.is_empty());

        let cached_base = cache.get("sys/sessions/web/base.jsonl").unwrap();
        assert!(String::from_utf8_lossy(&cached_base).contains("m1"));
        let cached_log = cache.get("sys/sessions/web/log-00.jsonl").unwrap();
        assert!(String::from_utf8_lossy(&cached_log).contains("m3"));
    }

    #[test]
    fn over_budget_triggers_l4_tail_only() {
        let fake = FakeStore::new();
        // base 2 KB > session_max_bytes (1024) → L3 trips → L4 tail.
        let base = "x".repeat(2048);
        // log-00 is well-formed JSONL; tail of last 64 bytes must parse.
        let mut log = String::new();
        for i in 0..40 {
            log.push_str(&jsonl_line(&format!("m{:03}", i)));
            log.push('\n');
        }
        fake.seed("sys/sessions/web/base.jsonl", base.into_bytes());
        fake.seed("sys/sessions/web/log-00.jsonl", log.into_bytes());

        let store: Arc<dyn ObjectStore> = Arc::new(fake);
        let cache = CloudCache::new();

        let result = boot_restore(&store, &cache, &cfg()).unwrap();
        assert_eq!(result.warnings.len(), 1);
        match &result.warnings[0].kind {
            BootWarningKind::Truncated {
                kept_size,
                original_size,
            } => {
                assert!(*kept_size > 0);
                assert!(*kept_size <= 64);
                assert!(*original_size > 64);
            }
            other => panic!("expected Truncated, got {:?}", other),
        }
        // Base intentionally NOT in cache.
        assert!(cache.get("sys/sessions/web/base.jsonl").is_none());
        // Log present (just the tail).
        let log_kept = cache.get("sys/sessions/web/log-00.jsonl").unwrap();
        assert!(log_kept.len() <= 64);
    }

    #[test]
    fn l4_get_range_failure_promotes_to_l5_quarantine() {
        let fake = Arc::new(FakeStore::new());
        let base = "x".repeat(2048); // forces L3 trip
        let log = "y".repeat(2048); // also large
        fake.seed("sys/sessions/web/base.jsonl", base.into_bytes());
        fake.seed("sys/sessions/web/log-00.jsonl", log.into_bytes());
        fake.fail_get_range("sys/sessions/web/log-00.jsonl");

        let store: Arc<dyn ObjectStore> = fake.clone();
        let cache = CloudCache::new();

        let result = boot_restore(&store, &cache, &cfg()).unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert!(matches!(
            result.warnings[0].kind,
            BootWarningKind::Quarantined
        ));
        // Originals deleted, copies live under .quarantine/ (kept via the
        // direct Arc<FakeStore> handle to avoid downcasting the trait obj).
        let deletes = fake.deletes();
        assert!(deletes.iter().any(|k| k == "sys/sessions/web/base.jsonl"));
        assert!(deletes.iter().any(|k| k == "sys/sessions/web/log-00.jsonl"));
        let puts = fake.puts();
        assert!(puts
            .iter()
            .any(|(k, _)| k == "sys/sessions/web/.quarantine/base.jsonl"));
    }

    #[test]
    fn corrupt_tail_promotes_to_l5_quarantine() {
        let fake = FakeStore::new();
        let base = "x".repeat(2048); // L3 trips
        let log = "this is not json at all\n".repeat(100); // tail won't parse
        fake.seed("sys/sessions/web/base.jsonl", base.into_bytes());
        fake.seed("sys/sessions/web/log-00.jsonl", log.into_bytes());

        let store: Arc<dyn ObjectStore> = Arc::new(fake);
        let cache = CloudCache::new();

        let result = boot_restore(&store, &cache, &cfg()).unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert!(matches!(
            result.warnings[0].kind,
            BootWarningKind::Quarantined
        ));
    }

    #[test]
    fn heartbeat_conflict_returns_other_device_id() {
        let fake = FakeStore::new();
        let other_id = "device-zzzzzz";
        let hb = serde_json::to_vec(&serde_json::json!({
            "device_id": other_id,
            "ts": now_unix() - 10,
        }))
        .unwrap();
        fake.seed("sys/.heartbeat", hb);

        let store: Arc<dyn ObjectStore> = Arc::new(fake);
        let cache = CloudCache::new();

        let result = boot_restore(&store, &cache, &cfg()).unwrap();
        assert_eq!(result.heartbeat_conflict.as_deref(), Some(other_id));
    }

    #[test]
    fn stale_heartbeat_does_not_conflict() {
        let fake = FakeStore::new();
        let hb = serde_json::to_vec(&serde_json::json!({
            "device_id": "ancient",
            "ts": now_unix() - 99_999, // way past heartbeat_stale_secs
        }))
        .unwrap();
        fake.seed("sys/.heartbeat", hb);

        let store: Arc<dyn ObjectStore> = Arc::new(fake);
        let cache = CloudCache::new();

        let result = boot_restore(&store, &cache, &cfg()).unwrap();
        assert!(result.heartbeat_conflict.is_none());
    }

    #[test]
    fn memory_and_cron_restored_into_cache() {
        let fake = FakeStore::new();
        fake.seed("sys/MEMORY.md", b"## [m1] note\n".to_vec());
        fake.seed("sys/cron.json", b"{\"jobs\":[]}".to_vec());

        let store: Arc<dyn ObjectStore> = Arc::new(fake);
        let cache = CloudCache::new();

        boot_restore(&store, &cache, &cfg()).unwrap();
        assert_eq!(
            cache.get("sys/MEMORY.md").unwrap(),
            b"## [m1] note\n".to_vec()
        );
        assert_eq!(
            cache.get("sys/cron.json").unwrap(),
            b"{\"jobs\":[]}".to_vec()
        );
    }

    #[test]
    fn unique_chat_ids_filters_quarantine_and_dotfiles() {
        let keys = vec![
            "sys/sessions/web/base.jsonl".to_string(),
            "sys/sessions/web/log-00.jsonl".to_string(),
            "sys/sessions/web/.quarantine/old.jsonl".to_string(),
            "sys/sessions/telegram-1/base.jsonl".to_string(),
            "sys/sessions/.something/oops".to_string(),
        ];
        let ids = unique_chat_ids(&keys);
        assert_eq!(ids, vec!["telegram-1".to_string(), "web".to_string()]);
    }
}
