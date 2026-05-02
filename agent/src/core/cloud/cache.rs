//! PSRAM-backed in-memory cache for Tier 1 paths.
//!
//! Holds the working set of agent state (sessions, MEMORY.md, cron.json,
//! identity files, config). When cloud is enabled, all Tier 1 reads
//! come from here; writes update here first, then are routed through
//! the replicator (eager) or directly to S3 (strict).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct CloudCache {
    inner: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl CloudCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.lock().ok()?.get(key).cloned()
    }

    pub fn put(&self, key: &str, bytes: Vec<u8>) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(key.to_string(), bytes);
        }
    }

    pub fn delete(&self, key: &str) -> bool {
        self.inner.lock().map(|mut g| g.remove(key).is_some()).unwrap_or(false)
    }

    pub fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        match self.inner.lock() {
            Ok(g) => g.keys().filter(|k| k.starts_with(prefix)).cloned().collect(),
            Err(_) => vec![],
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.inner.lock().map(|g| g.values().map(|v| v.len()).sum()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get_roundtrips() {
        let c = CloudCache::new();
        c.put("sys/MEMORY.md", b"hello world".to_vec());
        assert_eq!(c.get("sys/MEMORY.md"), Some(b"hello world".to_vec()));
    }

    #[test]
    fn get_missing_returns_none() {
        let c = CloudCache::new();
        assert_eq!(c.get("sys/missing"), None);
    }

    #[test]
    fn put_overwrites() {
        let c = CloudCache::new();
        c.put("k", b"a".to_vec());
        c.put("k", b"bb".to_vec());
        assert_eq!(c.get("k"), Some(b"bb".to_vec()));
    }

    #[test]
    fn delete_removes() {
        let c = CloudCache::new();
        c.put("k", b"v".to_vec());
        assert!(c.delete("k"));
        assert_eq!(c.get("k"), None);
    }

    #[test]
    fn delete_missing_returns_false() {
        let c = CloudCache::new();
        assert!(!c.delete("k"));
    }

    #[test]
    fn keys_with_prefix_filters() {
        let c = CloudCache::new();
        c.put("sys/sessions/web/base.jsonl", vec![]);
        c.put("sys/sessions/web/log-00.jsonl", vec![]);
        c.put("sys/MEMORY.md", vec![]);
        let mut keys = c.keys_with_prefix("sys/sessions/web/");
        keys.sort();
        assert_eq!(keys, vec![
            "sys/sessions/web/base.jsonl".to_string(),
            "sys/sessions/web/log-00.jsonl".to_string(),
        ]);
    }

    #[test]
    fn total_bytes_sums_values() {
        let c = CloudCache::new();
        c.put("a", vec![0u8; 100]);
        c.put("b", vec![0u8; 200]);
        assert_eq!(c.total_bytes(), 300);
    }

    #[test]
    fn clone_shares_underlying_storage() {
        // Critical: CloudCache is Clone via Arc; clones must share state
        let c1 = CloudCache::new();
        let c2 = c1.clone();
        c1.put("k", b"v".to_vec());
        assert_eq!(c2.get("k"), Some(b"v".to_vec()));
    }
}
