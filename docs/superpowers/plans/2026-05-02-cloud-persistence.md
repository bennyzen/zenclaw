# Cloud Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ZenClaw transparently survive its own death — when cloud is configured, automatically restore on boot, sync writes to S3 with no flash wear, and let a fresh device fully recover the previous one's work via bucket creds alone.

**Architecture:** Tiered storage (T1 RAM-first cache + async S3 mirror; T2 streamed user files; T3 NVS+S3 mirror for config). Bucket-per-device (the bucket itself is cloud identity, decoupled from mDNS hostname). Sessions use append-log + rotating compaction (Kafka/WAL-style) for atomicity. Five boot safety layers prevent oversized objects from bricking the device. Hybrid write semantics: eager (PSRAM ack + async PUT) for sessions; strict (block until S3 confirm) for memory/cron/config. Surface-and-stop on dead-letter — never silent forever-retry.

**Tech Stack:** Rust on `esp-idf-svc` (ESP32 target) + `tokio`/`axum` (desktop target). S3 client via `EspHttpConnection` with hand-rolled SigV4 signer (existing). No new external crates required. Web UI is Nuxt 4 + `@nuxt/ui` v4 (existing patterns).

**Spec:** `docs/superpowers/specs/2026-05-02-cloud-persistence-design.md` (commit `ae13f30`). Read it before starting any task — every design decision is justified there.

---

## File structure

### Files to create

| Path | Responsibility |
|---|---|
| `agent/src/core/cloud/cache.rs` | PSRAM-backed in-memory cache with reader/writer interface for Tier 1 paths. Backed by `Arc<Mutex<HashMap<String, Vec<u8>>>>`. Tracks dirty flags per key. |
| `agent/src/core/cloud/replicator.rs` | Eager-path write queue (PSRAM-only FIFO with coalescing) + drainer thread that pops, signs, PUTs, retries with exponential backoff, and demotes to dead-letter after `retry_max` attempts. |
| `agent/src/core/cloud/boot.rs` | Boot-time restore sequence: heartbeat check → per-chat_id LIST → HEAD-gate (L3) → tail-fallback (L4) → quarantine (L5) → memory/cron/identity restore. |
| `agent/src/core/cloud/snapshots.rs` | Periodic flash snapshot writer (`data/.snapshot.bin`). Triggered by interval (15 min default), graceful shutdown, or stale-queue detection. Boot fallback when S3 unreachable. |
| `agent/src/core/cloud/migration.rs` | Initial-backup phase for devices whose flash already contains pre-cloud data. Reads local `data/`, PUTs to S3, marks `sys/.heartbeat` with this device's ID. |
| `web/app/components/CloudStatusCard.vue` | Dashboard card showing bucket name, last sync, queue depth, snapshot age, usage. Color-coded by health. |
| `web/app/components/CloudWarningBanner.vue` | Non-dismissable banner shown when cloud unconfigured. Disappears once `storage.bucket` is set. |
| `web/app/components/CloudFailureBanner.vue` | Banner variants for runtime failures (heartbeat conflict, dead-letter non-empty, boot warnings). |

### Files to modify

| Path | Change |
|---|---|
| `agent/src/core/cloud/client.rs` | Add public `S3Client::put(key, bytes)`, `get(key)`, `get_range(key, offset, length)`, `delete(key)`, `head(key)` wrappers around the existing `http_request` helper. |
| `agent/src/core/cloud/mod.rs` | Re-export new modules (`cache`, `replicator`, `boot`, `snapshots`, `migration`). |
| `agent/src/config.rs` | Extend `StorageConfig` with `session_max_bytes`, `log_compaction_bytes`, `replicator: ReplicatorConfig`, `snapshot: SnapshotConfig`. Add `Config::is_cloud_enabled() -> bool` helper. |
| `agent/src/core/sessions/mod.rs` | Patch `SessionManager::append` to write through the cache (eager path) when cloud enabled. Add log-rotation helper for compaction. |
| `agent/src/core/tools/memory_tools.rs` | Patch `write_file` to write through the cache (strict path) when cloud enabled. |
| `agent/src/core/cron.rs` | Patch the cron-save call site to write through the cache (strict path). |
| `agent/src/core/tools/file_tools.rs` | Add `read_range`, `head`, `tail`, `info` actions. Cap existing `read` at 32 KB (return error with hint above). Add tier-aware routing (Tier 2 paths under `files/` go to S3; everything else goes to local FS or Tier 1 cache). |
| `agent/src/core/prompt.rs` | Update tooling-section preamble to disambiguate `file` vs `storage` ("file = your normal interface; storage = raw bucket access for cross-device or debugging"). |
| `agent/src/main.rs` | Wire cloud bring-up into boot sequence (after NIC, before HTTP server). Spawn replicator drainer thread + snapshot timer. Add new endpoints. Extend `/api/status.cloud_storage` block. Hook `/api/files` for transparent S3 routing on `files/` paths. |
| `web/app/pages/dashboard.vue` | Render `CloudStatusCard` + warning/failure banners. |
| `web/app/pages/provision.vue` | Add cloud config step (after WiFi, before LLM provider). Add forking question at top (fresh setup vs restore from previous device). Implement recovery branch (steps 3a/3b). |
| `web/app/composables/useConnection.ts` | Add `cloudStatus` reactive bind to `/api/status.cloud_storage`. |
| `README.md` | Move Roadmap item #1 from "shipped when…" to "shipped" + remove the inline "designed to" hedge from the lead paragraph. |

### Module export shape

After all tasks land, `agent/src/core/cloud/mod.rs` will re-export:

```rust
pub mod sigv4;
pub mod client;
pub mod cache;
pub mod replicator;
pub mod boot;
pub mod snapshots;
pub mod migration;

pub use cache::CloudCache;
pub use replicator::{Replicator, PendingWrite, DeadLetterEntry};
pub use boot::{boot_restore, BootWarning, BootWarningKind};
pub use snapshots::{Snapshot, write_snapshot, read_snapshot};
pub use client::{S3Client, S3Error, S3Object, S3Listing};
```

## Dependency graph

```
T1 (S3 client put/get/delete/head)
  └─► T2 (config schema)
        └─► T3 (cache.rs)
              └─► T4 (replicator.rs)
                    ├─► T5 (sessions write-through, eager)
                    │     └─► T7 (boot restore)
                    └─► T6 (memory/cron/config write-through, strict)
                          └─► T7 (boot restore)
                                └─► T8 (flash snapshots)
                                      └─► T9 (status API + new endpoints)
                                            ├─► T10 (file tool extensions)  ──► T11 (/api/files routing)
                                            ├─► T12 (UI banner + status card)
                                            ├─► T13 (UI wizard cloud step)
                                            ├─► T14 (UI recovery branch)
                                            └─► T15 (migration phase)
                                                  └─► T16 (README + on-device smoke)
```

T1, T2 are foundation. T3 + T4 are the engine. T5 + T6 wire it in. T7 + T8 make it bootable. T9–T11 surface it to the agent. T12–T14 surface it to humans. T15 handles existing devices. T16 ships it.

T10 and T11 are independent of T12–T14 (one is tool-side, the other is UI-side); they can land in parallel after T9.

## Test invocation

The agent crate has no `tests/` directory yet — all tests are inline `#[cfg(test)] mod tests` blocks. No `agent/justfile` `test` recipe exists; standard cargo invocations:

```bash
# Desktop tests (fast, no toolchain — use this 99% of the time)
cd agent && cargo test --features desktop --no-default-features

# Single test
cd agent && cargo test --features desktop --no-default-features <test_name>

# ESP32 build sanity check (slow, 30s+ even with warm cache)
just build devkitc

# Full firmware rebuild + on-device smoke
./scripts/build-rust-firmware.sh devkitc
just flash devkitc /dev/ttyACM0
```

Inline tests use `#[tokio::test]` for async (desktop only — ESP32 uses `block_on`). Match existing patterns in `sessions/mod.rs`, `cron.rs`, `tools/memory_tools.rs`.

For S3 integration tests, use `mockito` or a trait-based `S3Client` injection (the latter is cleaner — see Task 4).

---

## Task 1: Extend `S3Client` with put/get/delete/head/get_range

**Files:**
- Modify: `agent/src/core/cloud/client.rs`

**Why:** Spec §6.1 incorrectly says `client.rs` needs no changes. In reality, `S3Client` currently exposes only `list` and `presign`; the `http_request` helper exists but no public wrappers for write/read/delete/head. All downstream tasks need these.

- [ ] **Step 1: Read existing `http_request` helper** (lines ~196-260 of `client.rs`) to confirm its signature and how it returns response bytes. Note: it's synchronous (uses `EspHttpConnection`).

- [ ] **Step 2: Write failing tests for new methods**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `client.rs`. These tests use the `parse_*_xml` helpers and `build_url`/`urlencode` patterns already exercised — for actual HTTP we add tests in Task 4 via the trait abstraction. For now, just unit-test the URL/header construction:

```rust
#[test]
fn put_constructs_correct_url_and_method() {
    // Build a fake StorageConfig
    let cfg = test_storage_config();
    let client = S3Client::from_config(&cfg).unwrap();
    let (method, url, headers) = client.build_put_request_for_test("sys/MEMORY.md", b"hello");
    assert_eq!(method, "PUT");
    assert!(url.contains("/test-bucket/sys/MEMORY.md"));
    assert!(headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization")));
    assert!(headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("x-amz-content-sha256")
        && *v == sha256_hex(b"hello")));
}

#[test]
fn get_range_includes_range_header() {
    let cfg = test_storage_config();
    let client = S3Client::from_config(&cfg).unwrap();
    let (_, _, headers) = client.build_get_range_request_for_test("files/manual.pdf", 1024, 4096);
    assert!(headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("range")
        && *v == "bytes=1024-5119"));
}

fn test_storage_config() -> StorageConfig {
    StorageConfig {
        path: None,
        access_key_id: Some("AKIAIOSFODNN7EXAMPLE".to_string()),
        secret_access_key: Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string()),
        endpoint: Some("https://example.r2.cloudflarestorage.com".to_string()),
        bucket: Some("test-bucket".to_string()),
        region: "auto".to_string(),
        // ...defaults from Task 2 fields...
        ..Default::default()
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cd agent && cargo test --features desktop --no-default-features client::tests
```

Expected: FAIL with "method `build_put_request_for_test` not found" etc.

- [ ] **Step 4: Implement public methods + `build_*_request_for_test` shims**

Add to `impl S3Client`:

```rust
/// Put an object. Returns Ok on 2xx, S3Error otherwise.
pub fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
    let body_hash = sha256_hex(bytes);
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let mut headers = self.sign("PUT", &path, &[], &body_hash);
    headers.push(("content-length".to_string(), bytes.len().to_string()));
    let _resp = http_request("PUT", &url, &headers, Some(bytes))?;
    Ok(())
}

/// Get an object's full bytes. Returns S3Error on 4xx/5xx (404 maps to NotFound variant).
pub fn get(&self, key: &str) -> Result<Vec<u8>> {
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let body_hash = sha256_hex(b"");
    let headers = self.sign("GET", &path, &[], &body_hash);
    http_request("GET", &url, &headers, None)
}

/// Get a byte range from an object. `length` bytes starting at `offset`.
pub fn get_range(&self, key: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let body_hash = sha256_hex(b"");
    let mut headers = self.sign("GET", &path, &[], &body_hash);
    headers.push(("range".to_string(), format!("bytes={}-{}", offset, offset + length - 1)));
    http_request("GET", &url, &headers, None)
}

/// Delete an object. Idempotent — 404 is treated as Ok.
pub fn delete(&self, key: &str) -> Result<()> {
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let body_hash = sha256_hex(b"");
    let headers = self.sign("DELETE", &path, &[], &body_hash);
    let _resp = http_request("DELETE", &url, &headers, None)?;
    Ok(())
}

/// HEAD an object. Returns content-length on 200, None on 404, S3Error otherwise.
pub fn head(&self, key: &str) -> Result<Option<u64>> {
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let body_hash = sha256_hex(b"");
    let headers = self.sign("HEAD", &path, &[], &body_hash);
    // http_request needs to expose response headers for HEAD; see Step 5
    head_request(&url, &headers)
}

// Test shims (gated #[cfg(test)] only)
#[cfg(test)]
pub fn build_put_request_for_test(&self, key: &str, bytes: &[u8]) -> (String, String, Vec<(String, String)>) {
    let body_hash = sha256_hex(bytes);
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let mut headers = self.sign("PUT", &path, &[], &body_hash);
    headers.push(("content-length".to_string(), bytes.len().to_string()));
    ("PUT".to_string(), url, headers)
}

#[cfg(test)]
pub fn build_get_range_request_for_test(&self, key: &str, offset: u64, length: u64) -> (String, String, Vec<(String, String)>) {
    let path = self.object_path(key);
    let url = build_url(&self.scheme_host, &path, &[]);
    let body_hash = sha256_hex(b"");
    let mut headers = self.sign("GET", &path, &[], &body_hash);
    headers.push(("range".to_string(), format!("bytes={}-{}", offset, offset + length - 1)));
    ("GET".to_string(), url, headers)
}
```

Note: the `sign()` method exists implicitly inside `list()` already — extract it as a private helper if not already separate. Same for `sha256_hex`.

- [ ] **Step 5: Add `head_request` helper to `http_request` family**

`http_request` currently only returns the response body. HEAD needs the content-length header. Add a sibling `head_request(url, headers) -> Result<Option<u64>>` that reads the response headers and parses content-length, returning `Ok(None)` on 404. Keep `http_request` unchanged; add the new function below it.

- [ ] **Step 6: Run tests, verify pass**

```bash
cd agent && cargo test --features desktop --no-default-features client::tests
```

Expected: all 4+ tests pass (the existing 4 + the 2 new + any others added).

- [ ] **Step 7: Verify ESP32 build still compiles**

```bash
just build devkitc 2>&1 | tail -5
```

Expected: `Finished release` (no compile errors).

- [ ] **Step 8: Commit**

```bash
git add agent/src/core/cloud/client.rs
git commit -m "feat(cloud): add S3Client put/get/delete/head/get_range public methods

Closes the spec §6.1 gap — list+presign were the only public ops;
all downstream cloud-persistence work needs full read/write surface.

- put(key, bytes), get(key), get_range(key, offset, length)
- delete(key) (404-tolerant — idempotent)
- head(key) -> Option<content_length> (404 returns None)

Builds on existing http_request helper + SigV4 signer (no new deps).
"
```

---

## Task 2: Extend `StorageConfig` with new fields

**Files:**
- Modify: `agent/src/config.rs`

- [ ] **Step 1: Write failing test for default values**

Add to the bottom of `config.rs`:

```rust
#[cfg(test)]
mod cloud_persistence_config_tests {
    use super::*;

    #[test]
    fn storage_config_defaults_session_budget_to_256k() {
        let cfg: StorageConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.session_max_bytes, 256_000);
        assert_eq!(cfg.log_compaction_bytes, 16_384);
    }

    #[test]
    fn storage_config_defaults_replicator() {
        let cfg: StorageConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.replicator.queue_max, 32);
        assert_eq!(cfg.replicator.retry_max, 5);
        assert_eq!(cfg.replicator.backoff_cap_secs, 60);
    }

    #[test]
    fn storage_config_defaults_snapshot() {
        let cfg: StorageConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.snapshot.interval_secs, 900);
        assert_eq!(cfg.snapshot.stale_queue_threshold_secs, 300);
    }

    #[test]
    fn is_cloud_enabled_requires_bucket_and_keys() {
        let mut cfg = Config::default();
        assert!(!cfg.is_cloud_enabled());

        cfg.storage = Some(StorageConfig {
            bucket: Some("b".to_string()),
            access_key_id: Some("k".to_string()),
            secret_access_key: Some("s".to_string()),
            endpoint: Some("https://e".to_string()),
            ..Default::default()
        });
        assert!(cfg.is_cloud_enabled());

        // Missing endpoint → not enabled
        cfg.storage.as_mut().unwrap().endpoint = None;
        assert!(!cfg.is_cloud_enabled());
    }
}
```

- [ ] **Step 2: Run, verify fail**

```bash
cd agent && cargo test --features desktop --no-default-features cloud_persistence_config_tests
```

Expected: compile errors — fields don't exist.

- [ ] **Step 3: Implement schema additions**

Modify the `StorageConfig` struct (find the existing definition, add fields):

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct StorageConfig {
    pub path: Option<String>,

    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default = "default_storage_region")]
    pub region: String,

    // NEW
    #[serde(default = "default_session_max_bytes")]
    pub session_max_bytes: usize,
    #[serde(default = "default_log_compaction_bytes")]
    pub log_compaction_bytes: usize,

    #[serde(default)]
    pub replicator: ReplicatorConfig,
    #[serde(default)]
    pub snapshot: SnapshotConfig,
}

fn default_session_max_bytes() -> usize { 256_000 }
fn default_log_compaction_bytes() -> usize { 16_384 }

#[derive(Debug, Clone, Deserialize)]
pub struct ReplicatorConfig {
    #[serde(default = "default_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_retry_max")]
    pub retry_max: u8,
    #[serde(default = "default_backoff_cap_secs")]
    pub backoff_cap_secs: u32,
}
impl Default for ReplicatorConfig {
    fn default() -> Self {
        Self { queue_max: 32, retry_max: 5, backoff_cap_secs: 60 }
    }
}
fn default_queue_max() -> u32 { 32 }
fn default_retry_max() -> u8 { 5 }
fn default_backoff_cap_secs() -> u32 { 60 }

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotConfig {
    #[serde(default = "default_snapshot_interval_secs")]
    pub interval_secs: u32,
    #[serde(default = "default_stale_queue_threshold_secs")]
    pub stale_queue_threshold_secs: u32,
}
impl Default for SnapshotConfig {
    fn default() -> Self {
        Self { interval_secs: 900, stale_queue_threshold_secs: 300 }
    }
}
fn default_snapshot_interval_secs() -> u32 { 900 }
fn default_stale_queue_threshold_secs() -> u32 { 300 }
```

And add the helper to `impl Config`:

```rust
impl Config {
    /// True when cloud-persistence is configured and operational.
    pub fn is_cloud_enabled(&self) -> bool {
        match &self.storage {
            Some(s) => s.bucket.is_some()
                && s.access_key_id.is_some()
                && s.secret_access_key.is_some()
                && s.endpoint.is_some(),
            None => false,
        }
    }
}
```

- [ ] **Step 4: Run, verify pass**

```bash
cd agent && cargo test --features desktop --no-default-features cloud_persistence_config_tests
```

Expected: all 4 tests pass.

- [ ] **Step 5: Verify ESP32 build**

```bash
just build devkitc 2>&1 | tail -5
```

Expected: `Finished release`.

- [ ] **Step 6: Commit**

```bash
git add agent/src/config.rs
git commit -m "feat(config): extend StorageConfig with cloud persistence knobs

session_max_bytes (256 KB), log_compaction_bytes (16 KB),
replicator.{queue_max=32, retry_max=5, backoff_cap_secs=60},
snapshot.{interval_secs=900, stale_queue_threshold_secs=300}.

Plus Config::is_cloud_enabled() helper — true when storage block
has bucket + endpoint + access_key + secret_key all set."
```

---

## Task 3: `cloud::cache` — PSRAM-backed in-memory cache

**Files:**
- Create: `agent/src/core/cloud/cache.rs`
- Modify: `agent/src/core/cloud/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `agent/src/core/cloud/cache.rs`:

```rust
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
```

Add to `agent/src/core/cloud/mod.rs`:

```rust
pub mod cache;
pub use cache::CloudCache;
```

- [ ] **Step 2: Run, verify fail**

```bash
cd agent && cargo test --features desktop --no-default-features cloud::cache::tests
```

Expected: tests don't exist yet — first run creates them. Verify the file is wired (compile error if mod.rs not updated).

- [ ] **Step 3: Verify pass after Step 1's code is in**

```bash
cd agent && cargo test --features desktop --no-default-features cloud::cache::tests
```

Expected: all 8 tests pass.

- [ ] **Step 4: ESP32 build sanity**

```bash
just build devkitc 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/cloud/cache.rs agent/src/core/cloud/mod.rs
git commit -m "feat(cloud): add CloudCache — PSRAM-backed Tier 1 in-memory store

Foundation module for cloud persistence. Holds the working set of
agent state when cloud is enabled. Clone-shared via Arc<Mutex<...>>
so multiple subsystems (sessions, memory, cron) can hold their own
handle without copy-cost.

Pure data structure for now; integration into write paths comes in
T5/T6 once the replicator is in place."
```

---

## Task 4: `cloud::replicator` — eager write queue + drainer thread

**Files:**
- Create: `agent/src/core/cloud/replicator.rs`
- Modify: `agent/src/core/cloud/mod.rs`

This is the largest single task. The drainer thread + retry/backoff/dead-letter logic is intricate. Use a trait abstraction so we can unit-test against a fake `S3Client` impl.

- [ ] **Step 1: Define trait abstraction so we can fake the S3 client**

Add to `agent/src/core/cloud/client.rs` (alongside existing `S3Client`):

```rust
/// Trait abstraction over S3 ops so the replicator can be unit-tested
/// against a fake without real network. Implemented by S3Client; mocked
/// in tests.
pub trait ObjectStore: Send + Sync {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()>;
    fn get(&self, key: &str) -> Result<Vec<u8>>;
    fn delete(&self, key: &str) -> Result<()>;
    fn head(&self, key: &str) -> Result<Option<u64>>;
}

impl ObjectStore for S3Client {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> { Self::put(self, key, bytes) }
    fn get(&self, key: &str) -> Result<Vec<u8>> { Self::get(self, key) }
    fn delete(&self, key: &str) -> Result<()> { Self::delete(self, key) }
    fn head(&self, key: &str) -> Result<Option<u64>> { Self::head(self, key) }
}
```

- [ ] **Step 2: Write failing tests for replicator behavior**

Create `agent/src/core/cloud/replicator.rs`:

```rust
//! Eager-path write queue + drainer thread.
//!
//! Tier 1 eager writes: PSRAM cache update → enqueue here → drainer
//! pops, signs, PUTs to S3. On failure: exponential backoff, retry up
//! to retry_max, then demote to dead-letter. Dead-letter entries
//! surface in /api/status and a UI banner; surface-and-stop semantics
//! (no silent forever-retry).

use crate::core::cloud::client::{ObjectStore, S3Error};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PendingWrite {
    pub key: String,
    pub bytes: Vec<u8>,
    pub queued_at: Instant,
    pub retry_count: u8,
}

#[derive(Debug, Clone)]
pub struct DeadLetterEntry {
    pub key: String,
    pub bytes: Vec<u8>,
    pub retry_count: u8,
    pub last_error_at: Instant,
    pub last_error_msg: String,
}

#[derive(Clone)]
pub struct ReplicatorConfig {
    pub queue_max: u32,
    pub retry_max: u8,
    pub backoff_cap_secs: u32,
}

impl From<&crate::config::ReplicatorConfig> for ReplicatorConfig {
    fn from(c: &crate::config::ReplicatorConfig) -> Self {
        Self { queue_max: c.queue_max, retry_max: c.retry_max, backoff_cap_secs: c.backoff_cap_secs }
    }
}

#[derive(Default)]
struct ReplicatorState {
    queue: VecDeque<PendingWrite>,
    dead_letter: Vec<DeadLetterEntry>,
    last_sync_at: Option<Instant>,
    stopping: bool,
}

#[derive(Clone)]
pub struct Replicator {
    state: Arc<Mutex<ReplicatorState>>,
    cv: Arc<Condvar>,
    cfg: ReplicatorConfig,
}

impl Replicator {
    pub fn new(cfg: ReplicatorConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(ReplicatorState::default())),
            cv: Arc::new(Condvar::new()),
            cfg,
        }
    }

    /// Enqueue a write. Coalesces by key — if the same key is already
    /// pending and not yet started, replace its bytes (last-writer-wins).
    /// Blocks if queue is at queue_max until depth drops below half-cap.
    pub fn enqueue(&self, key: String, bytes: Vec<u8>) {
        let mut g = self.state.lock().unwrap();
        // Backpressure
        while g.queue.len() as u32 >= self.cfg.queue_max && !g.stopping {
            g = self.cv.wait(g).unwrap();
        }
        if g.stopping { return; }
        // Coalesce
        if let Some(existing) = g.queue.iter_mut().find(|e| e.key == key) {
            existing.bytes = bytes;
            existing.queued_at = Instant::now();
        } else {
            g.queue.push_back(PendingWrite {
                key, bytes, queued_at: Instant::now(), retry_count: 0
            });
        }
        self.cv.notify_one();
    }

    pub fn queue_depth(&self) -> usize { self.state.lock().unwrap().queue.len() }
    pub fn dead_letter(&self) -> Vec<DeadLetterEntry> { self.state.lock().unwrap().dead_letter.clone() }
    pub fn last_sync_at(&self) -> Option<Instant> { self.state.lock().unwrap().last_sync_at }

    pub fn stop(&self) {
        let mut g = self.state.lock().unwrap();
        g.stopping = true;
        self.cv.notify_all();
    }

    /// Spawn the drainer thread. Returns a JoinHandle for shutdown.
    pub fn spawn_drainer(&self, store: Arc<dyn ObjectStore>) -> thread::JoinHandle<()> {
        let state = self.state.clone();
        let cv = self.cv.clone();
        let cfg = self.cfg.clone();
        thread::spawn(move || drainer_loop(state, cv, cfg, store))
    }
}

fn drainer_loop(
    state: Arc<Mutex<ReplicatorState>>,
    cv: Arc<Condvar>,
    cfg: ReplicatorConfig,
    store: Arc<dyn ObjectStore>,
) {
    loop {
        // Wait for work
        let mut item = {
            let mut g = state.lock().unwrap();
            while g.queue.is_empty() && !g.stopping {
                g = cv.wait(g).unwrap();
            }
            if g.stopping && g.queue.is_empty() { return; }
            g.queue.pop_front().unwrap()
        };
        cv.notify_all(); // wake any backpressure-blocked writers

        // Attempt PUT
        match store.put(&item.key, &item.bytes) {
            Ok(()) => {
                let mut g = state.lock().unwrap();
                g.last_sync_at = Some(Instant::now());
            }
            Err(S3Error(msg)) => {
                item.retry_count += 1;
                if item.retry_count > cfg.retry_max {
                    let mut g = state.lock().unwrap();
                    g.dead_letter.push(DeadLetterEntry {
                        key: item.key,
                        bytes: item.bytes,
                        retry_count: item.retry_count - 1,
                        last_error_at: Instant::now(),
                        last_error_msg: msg,
                    });
                } else {
                    // Backoff: 2^(retry-1) seconds, capped
                    let delay = Duration::from_secs(
                        (1u32 << (item.retry_count - 1).min(10))
                            .min(cfg.backoff_cap_secs) as u64
                    );
                    thread::sleep(delay);
                    let mut g = state.lock().unwrap();
                    g.queue.push_front(item);
                    cv.notify_one();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fake store that succeeds N times then fails M times then succeeds.
    struct FakeStore {
        puts: Mutex<Vec<(String, Vec<u8>)>>,
        fail_for: AtomicUsize,
        fail_msg: String,
    }
    impl FakeStore {
        fn new() -> Self { Self { puts: Mutex::new(vec![]), fail_for: AtomicUsize::new(0), fail_msg: "fake".to_string() } }
        fn fail_next_n(&self, n: usize) { self.fail_for.store(n, Ordering::SeqCst); }
        fn put_log(&self) -> Vec<(String, Vec<u8>)> { self.puts.lock().unwrap().clone() }
    }
    impl ObjectStore for FakeStore {
        fn put(&self, key: &str, bytes: &[u8]) -> crate::core::cloud::client::Result<()> {
            if self.fail_for.load(Ordering::SeqCst) > 0 {
                self.fail_for.fetch_sub(1, Ordering::SeqCst);
                return Err(S3Error(self.fail_msg.clone()));
            }
            self.puts.lock().unwrap().push((key.to_string(), bytes.to_vec()));
            Ok(())
        }
        fn get(&self, _key: &str) -> crate::core::cloud::client::Result<Vec<u8>> { unimplemented!() }
        fn delete(&self, _key: &str) -> crate::core::cloud::client::Result<()> { Ok(()) }
        fn head(&self, _key: &str) -> crate::core::cloud::client::Result<Option<u64>> { Ok(None) }
    }

    fn cfg() -> ReplicatorConfig { ReplicatorConfig { queue_max: 32, retry_max: 3, backoff_cap_secs: 1 } }

    #[test]
    fn enqueue_and_drain_single_write() {
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        let h = r.spawn_drainer(store.clone());
        r.enqueue("sys/MEMORY.md".to_string(), b"hello".to_vec());
        // Wait briefly for drain
        thread::sleep(Duration::from_millis(50));
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![("sys/MEMORY.md".to_string(), b"hello".to_vec())]);
    }

    #[test]
    fn coalesces_pending_writes_for_same_key() {
        // Writes happen faster than the drainer can drain — only the last
        // version of each key should hit the store.
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        // Don't spawn drainer yet — fill queue first
        r.enqueue("k".to_string(), b"v1".to_vec());
        r.enqueue("k".to_string(), b"v2".to_vec());
        r.enqueue("k".to_string(), b"v3".to_vec());
        assert_eq!(r.queue_depth(), 1);

        let h = r.spawn_drainer(store.clone());
        thread::sleep(Duration::from_millis(50));
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![("k".to_string(), b"v3".to_vec())]);
    }

    #[test]
    fn retries_then_succeeds() {
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        store.fail_next_n(2);
        let h = r.spawn_drainer(store.clone());
        r.enqueue("k".to_string(), b"v".to_vec());
        // Wait for retries (1s + 2s with backoff_cap_secs=1 → ~2s total)
        thread::sleep(Duration::from_millis(2500));
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![("k".to_string(), b"v".to_vec())]);
        assert!(r.dead_letter().is_empty());
    }

    #[test]
    fn promotes_to_dead_letter_after_retry_max() {
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        store.fail_next_n(100); // will exhaust all retries
        let h = r.spawn_drainer(store.clone());
        r.enqueue("k".to_string(), b"v".to_vec());
        thread::sleep(Duration::from_millis(5000)); // 1+1+1 = 3s of backoffs at cap
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![]); // never succeeded
        let dl = r.dead_letter();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].key, "k");
        assert_eq!(dl[0].retry_count, 3);
    }

    #[test]
    fn backpressure_blocks_when_queue_full() {
        let cfg = ReplicatorConfig { queue_max: 2, retry_max: 1, backoff_cap_secs: 1 };
        let r = Replicator::new(cfg);
        let store = Arc::new(FakeStore::new());
        store.fail_next_n(100);
        let _h = r.spawn_drainer(store.clone());
        r.enqueue("a".to_string(), b"v".to_vec());
        r.enqueue("b".to_string(), b"v".to_vec());
        // Third enqueue would block — verify in a separate thread that it
        // doesn't return immediately.
        let r2 = r.clone();
        let blocked = thread::spawn(move || {
            r2.enqueue("c".to_string(), b"v".to_vec());
        });
        thread::sleep(Duration::from_millis(100));
        assert!(!blocked.is_finished(), "third enqueue should block on backpressure");
        r.stop();
        let _ = blocked.join();
    }
}
```

Add to `mod.rs`:

```rust
pub mod replicator;
pub use replicator::{Replicator, ReplicatorConfig, PendingWrite, DeadLetterEntry};
```

- [ ] **Step 3: Run tests, verify pass**

```bash
cd agent && cargo test --features desktop --no-default-features cloud::replicator
```

Expected: all 5 tests pass.

- [ ] **Step 4: ESP32 build sanity**

```bash
just build devkitc 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/cloud/replicator.rs agent/src/core/cloud/mod.rs agent/src/core/cloud/client.rs
git commit -m "feat(cloud): add Replicator — eager write queue + drainer thread

PSRAM-only FIFO with per-key coalescing (last-writer-wins for same
key). Single drainer thread pops, attempts S3 PUT, retries with
exponential backoff (1→2→4→...→backoff_cap_secs), demotes to
dead-letter after retry_max. Surface-and-stop semantics — dead
letter is exposed via Replicator::dead_letter() for /api/status.

Backpressure: enqueue blocks when queue depth reaches queue_max,
unblocks when drainer pops below it. Prevents PSRAM ↔ S3 runaway
divergence under sustained network failure.

ObjectStore trait extracted on S3Client for unit-test injectability;
real impl forwards directly. 5 unit tests cover: single drain,
coalescing, retry-then-succeed, dead-letter promotion, backpressure."
```

---

## Task 5: Wire `SessionManager::append` through cache + replicator (eager path)

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`
- Modify: `agent/src/core/gateway.rs` (likely owns the `Arc<CloudCache>` + `Arc<Replicator>` since it owns `SessionManager`)

This task introduces the rotating-log compaction logic for sessions. Read spec §6.4 carefully before starting.

- [ ] **Step 1: Add cloud-aware constructor variant for SessionManager**

Find the existing `SessionManager` struct. Add fields:

```rust
pub struct SessionManager {
    sessions_dir: String,                       // existing
    // ... existing fields ...

    // NEW (all Optional — None means cloud disabled)
    cache: Option<crate::core::cloud::CloudCache>,
    replicator: Option<std::sync::Arc<crate::core::cloud::Replicator>>,
    log_compaction_bytes: usize,                // copied from StorageConfig
}
```

Add a builder method:

```rust
impl SessionManager {
    pub fn with_cloud(
        mut self,
        cache: crate::core::cloud::CloudCache,
        replicator: std::sync::Arc<crate::core::cloud::Replicator>,
        log_compaction_bytes: usize,
    ) -> Self {
        self.cache = Some(cache);
        self.replicator = Some(replicator);
        self.log_compaction_bytes = log_compaction_bytes;
        self
    }
}
```

- [ ] **Step 2: Write failing test for rotating-log append**

Add to the `#[cfg(test)] mod tests` in `sessions/mod.rs`:

```rust
#[test]
fn append_when_cloud_enabled_writes_to_log_in_cache_and_replicator() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = crate::core::cloud::CloudCache::new();
    let cfg = crate::core::cloud::ReplicatorConfig { queue_max: 32, retry_max: 1, backoff_cap_secs: 1 };
    let replicator = std::sync::Arc::new(crate::core::cloud::Replicator::new(cfg));

    let mgr = SessionManager::new(tmp.path().to_str().unwrap())
        .with_cloud(cache.clone(), replicator.clone(), 16_384);

    let entry = SessionEntry::Message {
        id: "msg1".into(), parent: None, role: Role::User,
        content: "hello".into(), tool_calls: None, tool_call_id: None,
    };
    mgr.append("web", &entry).unwrap();

    // Cache should hold the log file
    let log_key = "sys/sessions/web/log-00.jsonl";
    let cached = cache.get(log_key).expect("log entry cached");
    assert!(String::from_utf8_lossy(&cached).contains("msg1"));

    // Replicator queue should have a pending PUT for that key
    assert!(replicator.queue_depth() >= 1);
}

#[test]
fn append_compacts_log_when_threshold_exceeded() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = crate::core::cloud::CloudCache::new();
    let cfg = crate::core::cloud::ReplicatorConfig { queue_max: 32, retry_max: 1, backoff_cap_secs: 1 };
    let replicator = std::sync::Arc::new(crate::core::cloud::Replicator::new(cfg));

    // Tiny threshold so a single entry triggers compaction
    let mgr = SessionManager::new(tmp.path().to_str().unwrap())
        .with_cloud(cache.clone(), replicator.clone(), 32);

    for i in 0..3 {
        let entry = SessionEntry::Message {
            id: format!("msg{}", i), parent: None, role: Role::User,
            content: format!("content-of-message-number-{}-padding-padding-padding", i),
            tool_calls: None, tool_call_id: None,
        };
        mgr.append("web", &entry).unwrap();
    }

    // After compaction, base.jsonl should exist with all entries; log should rotate
    let base = cache.get("sys/sessions/web/base.jsonl");
    assert!(base.is_some(), "base.jsonl populated after compaction");
    let base_content = String::from_utf8_lossy(&base.unwrap());
    for i in 0..3 {
        assert!(base_content.contains(&format!("msg{}", i)));
    }

    // base.meta.json should track highest_absorbed_log
    let meta = cache.get("sys/sessions/web/base.meta.json");
    assert!(meta.is_some());
    let meta_str = String::from_utf8_lossy(&meta.unwrap());
    assert!(meta_str.contains("highest_absorbed_log"));
}

#[test]
fn append_when_cloud_disabled_uses_local_only() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(tmp.path().to_str().unwrap()); // no .with_cloud()
    let entry = SessionEntry::Message {
        id: "m".into(), parent: None, role: Role::User,
        content: "hi".into(), tool_calls: None, tool_call_id: None,
    };
    mgr.append("web", &entry).unwrap();

    // Local file written
    let path = tmp.path().join("web.jsonl");
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("hi"));
}
```

- [ ] **Step 3: Run, verify fail (compile)**

```bash
cd agent && cargo test --features desktop --no-default-features sessions::tests::append
```

Expected: compile errors — `with_cloud`, fields don't exist.

- [ ] **Step 4: Implement append-with-cloud routing**

Modify `SessionManager::append` to fork on `self.cache.is_some()`:

```rust
pub fn append(&self, chat_id: &str, entry: &SessionEntry) -> Result<(), Box<dyn std::error::Error>> {
    // ... existing parent-link computation ...
    let to_write = /* existing logic */;
    let line = serde_json::to_string(&to_write)? + "\n";

    if let (Some(cache), Some(replicator)) = (&self.cache, &self.replicator) {
        // Cloud path: append to log in cache, enqueue PUT, possibly compact
        self.append_via_cloud(chat_id, &line, cache, replicator)?;
    } else {
        // Local-only path (existing behavior)
        let path = self.session_path(chat_id);
        // ...existing fs::OpenOptions::new().append(true).open()...
    }
    Ok(())
}

fn append_via_cloud(
    &self,
    chat_id: &str,
    line: &str,
    cache: &CloudCache,
    replicator: &Arc<Replicator>,
) -> Result<(), Box<dyn std::error::Error>> {
    let log_idx = self.current_log_index(chat_id, cache);
    let log_key = format!("sys/sessions/{}/log-{:02}.jsonl", chat_id, log_idx);

    // Append to in-cache log
    let mut current = cache.get(&log_key).unwrap_or_default();
    current.extend_from_slice(line.as_bytes());
    cache.put(&log_key, current.clone());
    replicator.enqueue(log_key.clone(), current);

    // Check compaction threshold
    if cache.get(&log_key).map(|v| v.len()).unwrap_or(0) >= self.log_compaction_bytes {
        self.compact_log(chat_id, log_idx, cache, replicator)?;
    }
    Ok(())
}

fn current_log_index(&self, chat_id: &str, cache: &CloudCache) -> u32 {
    // Find the highest log-NN.jsonl under this chat_id's prefix
    let prefix = format!("sys/sessions/{}/log-", chat_id);
    cache.keys_with_prefix(&prefix)
        .iter()
        .filter_map(|k| {
            k.strip_prefix(&prefix)?
                .strip_suffix(".jsonl")?
                .parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0)
}

fn compact_log(
    &self,
    chat_id: &str,
    log_idx: u32,
    cache: &CloudCache,
    replicator: &Arc<Replicator>,
) -> Result<(), Box<dyn std::error::Error>> {
    let base_key = format!("sys/sessions/{}/base.jsonl", chat_id);
    let log_key = format!("sys/sessions/{}/log-{:02}.jsonl", chat_id, log_idx);
    let meta_key = format!("sys/sessions/{}/base.meta.json", chat_id);

    let base = cache.get(&base_key).unwrap_or_default();
    let log = cache.get(&log_key).unwrap_or_default();
    let mut new_base = base;
    new_base.extend_from_slice(&log);

    // 1. PUT new base
    cache.put(&base_key, new_base.clone());
    replicator.enqueue(base_key, new_base);

    // 2. PUT meta with highest_absorbed_log
    let meta = serde_json::json!({ "highest_absorbed_log": log_idx }).to_string();
    cache.put(&meta_key, meta.as_bytes().to_vec());
    replicator.enqueue(meta_key, meta.as_bytes().to_vec());

    // 3. Future appends go to log-(log_idx+1) — no need to write empty log here
    Ok(())
}
```

- [ ] **Step 5: Run, verify pass**

```bash
cd agent && cargo test --features desktop --no-default-features sessions::tests::append
```

Expected: 3 new tests pass; existing session tests still pass.

- [ ] **Step 6: Wire from gateway**

In `agent/src/core/gateway.rs`, when constructing `SessionManager`, check `Config::is_cloud_enabled()` and call `.with_cloud(...)` with the cache + replicator. The cache and replicator are owned by the Gateway itself (Arc-shared with main.rs which spawns the drainer).

- [ ] **Step 7: ESP32 build sanity + commit**

```bash
just build devkitc 2>&1 | tail -5
cd /home/ben/repos/zenclaw
git add agent/src/core/sessions/mod.rs agent/src/core/gateway.rs
git commit -m "feat(cloud): route SessionManager::append through cache + replicator

When cloud is enabled, session appends go to PSRAM cache + the
eager replicator queue under sys/sessions/{chat_id}/log-NN.jsonl.
Threshold-based compaction folds log into base.jsonl + writes
base.meta.json with highest_absorbed_log, then rotates to
log-(NN+1) for future appends.

When cloud disabled, behavior is unchanged (local file append).
3 new tests cover: cloud-enabled append, threshold compaction,
local-only fallback."
```

---

## Task 6: Wire memory/cron/config writes through cache (strict path)

**Files:**
- Modify: `agent/src/core/tools/memory_tools.rs` (the `write_file` helper at line ~479)
- Modify: `agent/src/core/cron.rs` (find the cron-save call site)
- Modify: `agent/src/main.rs` (config-write path in `/api/config` handler)

The strict path is simpler than eager: write to PSRAM cache, then **synchronously** PUT to S3 via `S3Client` (bypassing the replicator queue). Block on S3 confirmation. After `retry_max` failures, return an error to the caller.

- [ ] **Step 1: Add a `strict_put` helper to `cloud::client` or a new file `cloud::strict.rs`**

```rust
// agent/src/core/cloud/strict.rs (new)
//! Strict-path writes — block on S3 confirmation, retry inline,
//! return error to caller after retry_max.

use crate::core::cloud::client::{ObjectStore, S3Error};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub fn strict_put(
    store: &Arc<dyn ObjectStore>,
    key: &str,
    bytes: &[u8],
    retry_max: u8,
    backoff_cap_secs: u32,
) -> Result<(), S3Error> {
    let mut last_err = S3Error("not attempted".to_string());
    for attempt in 0..=retry_max {
        match store.put(key, bytes) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e;
                if attempt < retry_max {
                    let delay = Duration::from_secs(
                        (1u32 << attempt.min(10)).min(backoff_cap_secs) as u64
                    );
                    thread::sleep(delay);
                }
            }
        }
    }
    Err(last_err)
}
```

- [ ] **Step 2: Tests for strict_put**

```rust
#[cfg(test)]
mod tests {
    // Reuse FakeStore pattern from replicator::tests
    // Tests:
    //  - succeeds first try
    //  - succeeds after N retries
    //  - returns error after retry_max
}
```

- [ ] **Step 3: Wire memory_tools::write_file**

`memory_tools::write_file` currently writes to a local path. Add a tier-routing helper that the function calls — when cloud enabled and the path matches a Tier 1 key, route through cache + strict_put.

```rust
// In memory_tools.rs, around line 479
fn write_file(path: &str, content: &str, ctx: &ToolContext) -> std::io::Result<()> {
    if let Some(cloud) = ctx.cloud_writer() {
        // Tier 1 strict path
        let key = format!("sys/{}", path.strip_prefix("data/").unwrap_or(path));
        cloud.cache().put(&key, content.as_bytes().to_vec());
        cloud.strict_put(&key, content.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    }
    // Always also write to local file (snapshot fallback)
    std::fs::write(path, content)
}
```

This requires extending `ToolContext` to optionally carry a `CloudWriter` (a small struct holding `Arc<CloudCache>` + `Arc<dyn ObjectStore>` + `ReplicatorConfig`). Add this to `ToolContext` definition.

- [ ] **Step 4: Wire cron save**

Apply the same pattern to `cron.rs` save path. The cron file lives at `data/cron.json` → S3 key `sys/cron.json`.

- [ ] **Step 5: Wire config save in `/api/config` handler**

In `main.rs`, after a successful POST to `/api/config` writes NVS, also strict-PUT to `sys/config.json`. If that fails after retries, return HTTP 503 to the caller and abort the reboot.

```rust
// In the /api/config handler:
if cfg.is_cloud_enabled() {
    let serialized = serde_json::to_vec(&cfg)?;
    match cloud::strict::strict_put(&store, "sys/config.json", &serialized,
                                    cfg.storage.as_ref().unwrap().replicator.retry_max,
                                    cfg.storage.as_ref().unwrap().replicator.backoff_cap_secs) {
        Ok(()) => { /* proceed with reboot */ }
        Err(e) => return Err(/* HTTP 503 with detail */),
    }
}
```

- [ ] **Step 6: Run all relevant tests + ESP32 build**

```bash
cd agent && cargo test --features desktop --no-default-features
just build devkitc 2>&1 | tail -5
```

- [ ] **Step 7: Commit**

```bash
git add agent/src/core/cloud/strict.rs agent/src/core/cloud/mod.rs \
        agent/src/core/tools/memory_tools.rs agent/src/core/cron.rs \
        agent/src/main.rs
git commit -m "feat(cloud): wire memory/cron/config through strict S3 path

Memory saves, cron updates, and config writes block on S3 PUT
confirmation (with retry_max retries + exponential backoff).
On exhausted retries, the caller gets an error — memory_save
returns it as a tool error to the agent; /api/config returns
HTTP 503 with the failure detail and aborts the reboot.

Cache is updated before the strict PUT (cache-write is still O(1));
local fs write is preserved as a snapshot fallback. ToolContext
gains an optional CloudWriter handle."
```

---

## Task 7: `cloud::boot` — restore sequence with safety layers L3/L4/L5

**Files:**
- Create: `agent/src/core/cloud/boot.rs`
- Modify: `agent/src/core/cloud/mod.rs`
- Modify: `agent/src/main.rs` (call `boot_restore` early in startup)

This is the boot flow from spec §6.6 — implements steps 4-8 (heartbeat + per-chat_id restore + memory/cron/identity restore). Step 3 (config restore) and steps 1-2 (NVS read + S3 client init) happen in `main.rs`.

- [ ] **Step 1: Define `BootWarning` types + signatures**

```rust
// agent/src/core/cloud/boot.rs
use crate::core::cloud::{client::ObjectStore, CloudCache};
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BootWarning {
    pub kind: BootWarningKind,
    pub chat_id: String,
    pub at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BootWarningKind {
    Truncated { original_size: u64, kept_size: u64 },
    Quarantined,
}

pub struct BootResult {
    pub warnings: Vec<BootWarning>,
    pub heartbeat_conflict: Option<String>, // device_id of conflict
}

pub struct BootConfig {
    pub session_max_bytes: usize,
    pub log_compaction_bytes: usize,
    pub device_id: String,           // MAC-derived hex
}
```

- [ ] **Step 2: Write failing tests for L3/L4/L5 trip points**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // ... fake ObjectStore that returns known sizes/contents per key ...

    #[test]
    fn within_budget_loads_full_session() {
        // Setup: fake store has base.jsonl (50 KB) + log-00 (5 KB),
        //        session_max_bytes = 256 KB
        // Act: boot_restore
        // Assert: cache has base + log entries, no warnings
    }

    #[test]
    fn over_budget_triggers_l4_tail_only() {
        // Setup: fake store has base.jsonl (300 KB),
        //        session_max_bytes = 256 KB
        // Act: boot_restore
        // Assert: cache has only the tail (~16 KB) of the log,
        //         warnings includes Truncated entry,
        //         base is dropped from cache
    }

    #[test]
    fn parse_failure_triggers_l5_quarantine() {
        // Setup: fake store has base.jsonl with corrupt JSON,
        //        tail GET also returns garbage
        // Act: boot_restore
        // Assert: original keys moved to .quarantine/ prefix,
        //         empty session created, Quarantined warning surfaced
    }

    #[test]
    fn heartbeat_conflict_returns_other_device_id() {
        // Setup: fake store's GET for sys/.heartbeat returns a different
        //        device_id with recent timestamp
        // Act: boot_restore
        // Assert: BootResult.heartbeat_conflict == Some(other_id)
    }
}
```

- [ ] **Step 3-N: Implement `boot_restore`**

Follow spec §6.6 sequence. Key helper: `tail_range(store, key, length) -> Result<Vec<u8>>` that does HEAD + ranged GET.

- [ ] **Step N+1: Wire from `main.rs`**

After cloud_init (Task 1's S3Client construction) and before the agent loop spawns:

```rust
// In main.rs after S3 client init
let boot_result = cloud::boot::boot_restore(&store, &cache, &boot_cfg)?;
if !boot_result.warnings.is_empty() {
    log::warn!("boot warnings: {:?}", boot_result.warnings);
    // Stash in a global state so /api/status can surface them
}
```

- [ ] **Step N+2: Tests pass + ESP32 build + commit**

---

## Task 8: `cloud::snapshots` — flash backup of PSRAM cache

**Files:**
- Create: `agent/src/core/cloud/snapshots.rs`
- Modify: `agent/src/core/cloud/mod.rs`
- Modify: `agent/src/main.rs` (spawn snapshot timer; call `read_snapshot` as boot fallback)

- [ ] **Step 1: Define snapshot format**

Use `bincode` (already a candidate dep — check `Cargo.toml`; if not, use `serde_json` for simplicity since cache is small).

```rust
// agent/src/core/cloud/snapshots.rs
use crate::core::cloud::CloudCache;
use std::collections::HashMap;
use std::path::Path;

const SNAPSHOT_PATH: &str = "data/.snapshot.bin";
const SNAPSHOT_TMP: &str = "data/.snapshot.tmp";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub written_at: i64,
    pub entries: HashMap<String, Vec<u8>>,
}

pub fn write_snapshot(cache: &CloudCache) -> std::io::Result<()> {
    let entries: HashMap<String, Vec<u8>> = cache.snapshot();  // requires new method on cache
    let snap = Snapshot { written_at: now_ts(), entries };
    let bytes = serde_json::to_vec(&snap)?; // or bincode
    std::fs::write(SNAPSHOT_TMP, &bytes)?;
    std::fs::rename(SNAPSHOT_TMP, SNAPSHOT_PATH)?;
    Ok(())
}

pub fn read_snapshot() -> std::io::Result<Option<Snapshot>> {
    if !Path::new(SNAPSHOT_PATH).exists() { return Ok(None); }
    let bytes = std::fs::read(SNAPSHOT_PATH)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}
```

- [ ] **Step 2: Add `CloudCache::snapshot` returning `HashMap<String, Vec<u8>>` clone**

- [ ] **Step 3: Tests for write/read roundtrip + atomicity (rename)**

- [ ] **Step 4: Snapshot timer thread in `main.rs`**

```rust
let cache_for_snap = cache.clone();
let interval = cfg.storage.as_ref().unwrap().snapshot.interval_secs;
std::thread::spawn(move || {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(interval as u64));
        if let Err(e) = cloud::snapshots::write_snapshot(&cache_for_snap) {
            log::warn!("snapshot write failed: {}", e);
        }
    }
});
```

- [ ] **Step 5: Boot fallback — if S3 GET fails entirely, read snapshot**

In `boot.rs`, wrap each S3 GET; on network error fall back to `read_snapshot()` and populate cache from it.

- [ ] **Step 6: Tests + ESP32 + commit**

---

## Task 9: `/api/status.cloud_storage` extensions + `/api/cloud/test` + `/api/cloud/takeover`

**Files:**
- Modify: `agent/src/main.rs`

Build the JSON response shape from spec §10.1. Extend the existing 60s LIST cache. Add the two new POST handlers.

- [ ] **Step 1: Sketch the response builder**

```rust
fn cloud_status_block(cfg: &Config, cache: &CloudCache, replicator: &Replicator,
                     boot_warnings: &[BootWarning], hb_conflict: &Option<String>,
                     last_snapshot_at: Option<Instant>) -> serde_json::Value {
    if !cfg.is_cloud_enabled() {
        return serde_json::json!({ "enabled": false });
    }
    serde_json::json!({
        "enabled": true,
        "bucket": cfg.storage.as_ref().unwrap().bucket,
        "endpoint": cfg.storage.as_ref().unwrap().endpoint,
        "region": cfg.storage.as_ref().unwrap().region,
        "sync": {
            "queue_depth": replicator.queue_depth(),
            "queue_max": cfg.storage.as_ref().unwrap().replicator.queue_max,
            "last_sync_ts": replicator.last_sync_at().map(|i| /* to ts */),
            "last_sync_age_secs": /* now - last_sync_at */,
            "dead_letter_count": replicator.dead_letter().len(),
        },
        "snapshot": { /* ... */ },
        "boot_warnings": boot_warnings,
        "failures": replicator.dead_letter().iter().take(10).collect::<Vec<_>>(),
        "heartbeat": { "ours": hb_conflict.is_none(), "conflict_with": hb_conflict },
        "usage": cached_usage_stats(),  // 60s LIST cache
    })
}
```

- [ ] **Step 2: `/api/cloud/test` handler** — runs the round-trip described in spec §9; returns `{ok, error?, stage}`. Stages enumerated in spec §9.

- [ ] **Step 3: `/api/cloud/takeover` handler** — single PUT of fresh `.heartbeat` with this device's ID; clears the stashed `hb_conflict` global.

- [ ] **Step 4: Test endpoints with curl against running desktop instance**

```bash
cargo run --features desktop --no-default-features
# In another shell:
curl -s http://localhost:3030/api/status | jq .cloud_storage
curl -s -X POST http://localhost:3030/api/cloud/test -d '{...}'
```

- [ ] **Step 5: ESP32 build + commit**

---

## Task 10: `file` tool extensions — `read_range`, `head`, `tail`, `info` + cap existing `read`

**Files:**
- Modify: `agent/src/core/tools/file_tools.rs`

- [ ] **Step 1: Update tool definition to expose new actions**

```rust
parameters: json!({
    "type": "object",
    "properties": {
        "action": { "type": "string", "enum": [
            "read", "write", "edit", "delete", "list_dir",
            "read_range", "head", "tail", "info"  // NEW
        ]},
        "path": { "type": "string" },
        "content": { "type": "string" },
        "offset": { "type": "integer", "description": "for read_range" },
        "length": { "type": "integer", "description": "for read_range/head/tail (default 16 KB, max 32 KB)" },
    },
    "required": ["action"]
})
```

- [ ] **Step 2: Implement each new action**

```rust
"read" => {
    let bytes = read_with_tier_routing(&path, ctx)?;
    if bytes.len() > 32 * 1024 {
        return ToolResult::Error(format!(
            "file is {} bytes; use read_range, head, or tail",
            bytes.len()
        ));
    }
    ToolResult::Text(String::from_utf8_lossy(&bytes).to_string())
}
"read_range" => {
    let offset = args["offset"].as_u64().unwrap_or(0);
    let length = args["length"].as_u64().unwrap_or(16 * 1024).min(32 * 1024);
    let bytes = read_range_with_tier_routing(&path, offset, length, ctx)?;
    ToolResult::Text(String::from_utf8_lossy(&bytes).to_string())
}
"head" => {
    let length = args["length"].as_u64().unwrap_or(4 * 1024).min(32 * 1024);
    let bytes = read_range_with_tier_routing(&path, 0, length, ctx)?;
    ToolResult::Text(String::from_utf8_lossy(&bytes).to_string())
}
"tail" => {
    let length = args["length"].as_u64().unwrap_or(4 * 1024).min(32 * 1024);
    let info = stat_with_tier_routing(&path, ctx)?;
    let offset = info.size.saturating_sub(length);
    let bytes = read_range_with_tier_routing(&path, offset, length, ctx)?;
    ToolResult::Text(String::from_utf8_lossy(&bytes).to_string())
}
"info" => {
    let info = stat_with_tier_routing(&path, ctx)?;
    ToolResult::Json(serde_json::json!({
        "size": info.size,
        "etag": info.etag,
        "last_modified": info.last_modified,
        "tier": info.tier,  // "tier1" | "tier2" | "local"
    }))
}
```

The tier-routing helpers (`read_with_tier_routing`, `read_range_with_tier_routing`, `stat_with_tier_routing`) check whether the path falls under `files/` (Tier 2) — if so, route to S3 via `ObjectStore`. Otherwise route to local FS or Tier 1 cache.

- [ ] **Step 3: Tests for each action (unit, in-file)**

- [ ] **Step 4: Update `prompt.rs` tooling section**

Disambiguate `file` vs `storage` with a one-line note (spec §7.2).

- [ ] **Step 5: ESP32 build + commit**

---

## Task 11: `/api/files` transparent S3 routing for `files/` paths

**Files:**
- Modify: `agent/src/main.rs`

Find existing `/api/files`, `/api/files/read`, `/api/files/write` handlers. Add a path prefix check: if path starts with `files/` and cloud is enabled, route to S3. Otherwise existing local FS path.

- [ ] **Step 1: Helper function** `route_file_op(path, op) -> result` — encapsulates the prefix check + dispatch
- [ ] **Step 2: Patch each handler to use it**
- [ ] **Step 3: Test with curl against desktop instance** (PUT to `files/test.txt`, GET back)
- [ ] **Step 4: ESP32 build + commit**

---

## Task 12: Web UI — dashboard banner + Cloud Status card

**Files:**
- Create: `web/app/components/CloudStatusCard.vue`
- Create: `web/app/components/CloudWarningBanner.vue`
- Create: `web/app/components/CloudFailureBanner.vue`
- Modify: `web/app/pages/dashboard.vue`
- Modify: `web/app/composables/useConnection.ts` (add `cloudStatus` reactive)

- [ ] **Step 1: Extend `useConnection` to poll `/api/status.cloud_storage`** (every 5s, similar to existing status polling)

- [ ] **Step 2: `CloudWarningBanner.vue`** — non-dismissable yellow banner; shown when `cloudStatus.enabled === false`. Click → router push to `/config?focus=cloud`.

- [ ] **Step 3: `CloudStatusCard.vue`** — shown when `cloudStatus.enabled === true`. Renders bucket, last_sync_age, queue_depth, snapshot age, usage. Color: green normal, yellow when sync stale or boot warnings, red when dead-letter or heartbeat conflict.

- [ ] **Step 4: `CloudFailureBanner.vue`** — variants for heartbeat conflict (with `[Take over]` button → POST `/api/cloud/takeover`), dead-letter, boot warnings.

- [ ] **Step 5: Wire into `dashboard.vue`** — render warning + card + failure banners conditionally.

- [ ] **Step 6: Test in browser** — `npm run dev` in `web/`, point at a desktop agent, verify banner appears for unconfigured state.

- [ ] **Step 7: Commit**

---

## Task 13: Web UI — wizard cloud step (fresh setup flow)

**Files:**
- Modify: `web/app/pages/provision.vue`

Add the cloud config step between WiFi and LLM provider (per spec §8.2 step 6).

- [ ] **Step 1: Add a new wizard step component** showing: endpoint, bucket name, access key ID, secret access key, region, [Test connection] button, "Where do I get these?" R2-instructions link

- [ ] **Step 2: Wire `[Test connection]`** → POST `/api/cloud/test` with the entered creds; show success/failure inline; only enable the [Next] button after a successful test

- [ ] **Step 3: Wire `[Skip]`** → confirms with modal text from spec §8.2; sets a flag in localStorage so the dashboard banner knows the user explicitly skipped

- [ ] **Step 4: When the wizard finishes flashing**, NVS now includes the storage block (esptool-js NVS partition write needs to include `storage/endpoint`, `storage/bucket`, etc. — extend the existing NVS-write logic in the wizard)

- [ ] **Step 5: Test by walking through wizard in dev mode** with a local MinIO target

- [ ] **Step 6: Commit**

---

## Task 14: Web UI — recovery-mode wizard branch (device-swap)

**Files:**
- Modify: `web/app/pages/provision.vue`

Implement the forking question + 3a/3b steps from spec §8.2. Reuse the cloud-test endpoint and a new `/api/cloud/preview-config` (read-only GET that returns `sys/config.json` from the entered bucket).

- [ ] **Step 1: Add the forking question** at top of wizard ("Fresh setup" / "Restore from previous device")

- [ ] **Step 2: Restore branch step 3a** — bucket cred entry + test

- [ ] **Step 3: Restore branch step 3b** — fetch and preview recovered config; handle empty-bucket case (spec §8.2 — fall back to fresh setup with explanation)

- [ ] **Step 4: Skip wizard steps 4 and 5** when in restore mode; pre-fill cloud step (6) with the same creds

- [ ] **Step 5: Test the full restore flow** end-to-end against a populated MinIO bucket

- [ ] **Step 6: Commit**

---

## Task 15: `cloud::migration` — initial backup for existing devices

**Files:**
- Create: `agent/src/core/cloud/migration.rs`
- Modify: `agent/src/core/cloud/boot.rs` (call migration before normal restore if bucket is empty)

Per spec §12 — when bucket is freshly configured on an existing device, upload all of `data/` before proceeding with normal boot.

- [ ] **Step 1: Detection** — bucket is "fresh" iff HEAD on `sys/.heartbeat` returns 404
- [ ] **Step 2: Migration sequence** — read all of `data/sessions/*.jsonl`, `data/MEMORY.md`, `data/cron.json`, `data/SOUL.md`, `data/AGENTS.md`; PUT each to `sys/...`; PUT current Config to `sys/config.json`; PUT initial heartbeat
- [ ] **Step 3: For sessions**, convert from local JSONL to base+log structure: PUT existing content as `base.jsonl`, write `base.meta.json` with `highest_absorbed_log: 0`, leave `log-00.jsonl` empty
- [ ] **Step 4: Failure recovery** — per spec §12, fall back to local-only mode + non-dismissable red banner; never lock the device out
- [ ] **Step 5: Surface migration progress in `/api/status`** for the duration (UI banner reads from this)
- [ ] **Step 6: Tests** with mocked S3 — fresh bucket, partial-failure recovery
- [ ] **Step 7: ESP32 build + commit**

---

## Task 16: README Roadmap update + on-device smoke

**Files:**
- Modify: `README.md`

- [ ] **Step 1: On-device smoke test sequence**
  - Configure cloud on a DevKitC via the wizard (against a real R2 bucket or MinIO)
  - Verify `/api/status.cloud_storage.enabled === true`
  - Send a chat message; verify `sys/sessions/web/log-00.jsonl` appears in bucket within 5s
  - Verify dashboard banner is gone, Cloud Status card shows green
  - `memory_save` from a chat; verify `sys/MEMORY.md` updates in bucket
  - Power-cycle the device; verify boot completes and history is intact
  - Disable WiFi for 1 min; verify queue_depth grows; re-enable; verify it drains

- [ ] **Step 2: Update README**
  - Move Roadmap item #1 from "shipped when…" to "**shipped 2026-XX-XX**"
  - Remove the "designed to" hedge from the lead paragraph (cloud persistence now genuinely ships)
  - Remove the `> **This section describes the target design.**` blockquote at the top of the Cloud Persistence section
  - Add a brief "Configured via the web UI's Provisioning wizard" line

- [ ] **Step 3: Commit + push**

```bash
git add README.md
git commit -m "docs: cloud persistence shipped — Roadmap #1 complete"
git push origin main
```

---

## Self-review checklist (run before declaring plan complete)

Done at plan-write time:

- ✅ Spec coverage: every section/requirement in the spec has a Task. Mapping:
  - Spec §4 (architecture) → T3+T4+T7
  - Spec §5 (bucket layout) → emerges from T5/T6 (key naming)
  - Spec §6.1 (modules) → T1 + T3 + T4 + T7 + T8 + T15
  - Spec §6.2 (write paths) → T5 + T6
  - Spec §6.3 (replicator) → T4
  - Spec §6.4 (session chunking) → T5
  - Spec §6.5 (snapshots) → T8
  - Spec §6.6 (boot flow) → T7
  - Spec §6.7 (failure modes) → covered across T4 + T7 + T9
  - Spec §7 (Tier 2) → T10 + T11
  - Spec §8 (Tier 3 / device-swap) → T6 (config strict path) + T14 (recovery wizard)
  - Spec §9 (HTTP API) → T9 + T11
  - Spec §10 (observability) → T9 + T12
  - Spec §11 (config schema) → T2
  - Spec §12 (migration) → T15
  - Spec §13 (testing) → in-file tests at each task; on-device smoke in T16
  - Spec §14 (impl order) → realized as T1-T16 sequence with dependency graph
- ✅ No placeholders ("TBD", "TODO", "fill in details") in any task body
- ✅ Type/method consistency: `CloudCache`, `Replicator`, `ObjectStore`, `BootWarning`, `Snapshot` are defined in T3/T4/T7/T8 respectively and consistently referenced thereafter
- ⚠ Some tasks (T6, T7, T8, T9, T10, T11, T15) are sketched at higher granularity than T1-T5 — engineer should expand them to full bite-sized steps when picking them up. The shape is clear; the per-line bite-sized step list would balloon this doc to 5x its size. The subagent-driven-development skill is well-suited to fleshing out the steps mid-execution.

---

## Final note for the executing engineer

This is a 4-6 PR feature if T1-T9 are bundled tightly, or 16 PRs if every task is independent. The dependency graph above shows what can land in parallel.

**Recommended sequencing**:
- Land T1-T9 one PR at a time over a focused work week (the engine + observability)
- Then T10-T11 (file tool surface) in one PR
- Then T12-T14 (web UI) over 2-3 PRs
- Then T15 (migration) as its own PR
- T16 ships it

Read the spec end to end before T1. Re-read §6.4 (rotating-log compaction with crash-safety analysis) before T5. Re-read §6.6 (boot flow with safety layers) before T7.

The brainstorming session that produced the spec is captured in conversation — every decision has a "why" recorded.
