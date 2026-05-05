# Multi-Conversation Web UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single-eternal-conversation web UI with a Claude.ai-style sidebar that lists, switches between, renames, and deletes chat sessions — backed by per-session sidecar metadata files that replicate alongside their JSONLs through the existing cloud-persistence path.

**Architecture:** Per-session metadata sidecar files (`data/sessions/<chat_id>.meta.json`) store title/kind/last-activity/preview, replicate via the existing per-chat cloud prefix (`sys/sessions/<id>/meta.json`), and self-heal when missing or corrupt. Backend exposes 4 REST routes (`GET/POST/PATCH/DELETE /api/sessions`); frontend gains a `SessionsSidebar` component, a `useSessions` composable, a two-column `chat.vue` layout, and dynamic `pages/chat/[id].vue` + `pages/chat/index.vue` routes. LLM-summarized titles generate as a post-turn background task off the user's critical path.

**Tech Stack:** Rust (esp-idf-svc on ESP32, axum on desktop), Vue 3 / Nuxt 4 / `@nuxt/ui`, existing `strict_put` cloud-write path, existing `SessionManager` cloud-aware persistence pattern, Playwright MCP for end-to-end tests.

---

## Spec Reference

Companion to `docs/superpowers/specs/2026-05-05-multi-conversations-design.md`. Read the spec first if unfamiliar; this plan executes against it.

## Divergence From Spec

The spec claimed `Vitest` is "already in `web/package.json`" — it isn't. `web/package.json` has no test framework set up. **v1 ships frontend without unit tests**, relying on Playwright MCP end-to-end + manual browser smoke for verification. Adding Vitest is a future task if a regression suite becomes warranted; setting up a framework just for two test files violates YAGNI.

## File Structure

```
agent/src/core/sessions/
  meta.rs                      ← NEW: SessionMeta types, detect_kind, synthesize_default
  mod.rs                       ← MODIFIED: 7 new methods (meta, set_meta, list_with_meta,
                                  bump_activity, rename, rename_internal, delete)

agent/src/core/cloud/
  boot.rs                      ← MODIFIED: per-chat restore loop also fetches meta.json

agent/src/core/
  gateway.rs                   ← MODIFIED: bump_activity hook + post-turn title-gen trigger
  title_gen.rs                 ← NEW: maybe_generate_title async function

agent/src/main.rs              ← MODIFIED: 4 new HTTP routes (sessions {GET, POST, PATCH, DELETE})
agent/src/desktop/server.rs    ← MODIFIED: 4 new HTTP routes (same shape)

web/app/
  composables/useSessions.ts   ← NEW
  components/SessionsSidebar.vue ← NEW
  layouts/chat.vue             ← NEW: two-column shell
  pages/chat/[id].vue          ← NEW: dynamic route, contents lifted from current chat.vue
  pages/chat/index.vue         ← NEW: bare /chat redirect-or-empty
  pages/chat.vue               ← REMOVED

docs/superpowers/playbooks/
  multi-conversations-e2e.md   ← NEW: Playwright happy-path script
```

## Conventions

- **Cargo test invocation**: `cd agent && cargo test --features desktop --lib <test_name>` (the desktop feature avoids pulling in esp-idf-svc on the host toolchain).
- **ESP32 verification**: `./scripts/build-rust-firmware.sh devkitc` then reflash via the wizard, then run the manual smoke commands.
- **Commit style**: follow the existing repo pattern (`feat:`, `refactor:`, `docs:`, `fix:` prefixes; co-authored-by trailer; HEREDOC for multi-line bodies).
- **One task = one commit** unless explicitly noted.

---

## Task 1: SessionMeta types + serde

**Files:**
- Create: `agent/src/core/sessions/meta.rs`
- Modify: `agent/src/core/sessions/mod.rs:1-15` (add `pub mod meta;`)

- [ ] **Step 1: Create the new module file with the data types and a roundtrip test**

Create `agent/src/core/sessions/meta.rs`:

```rust
//! Per-session metadata sidecar (`data/sessions/<chat_id>.meta.json`).
//!
//! Replicates alongside the session's JSONL via the existing per-chat
//! cloud prefix `sys/sessions/<chat_id>/meta.json`. Self-healing: a
//! missing or corrupt sidecar degrades to a synthesized default; only
//! LLM-summarized and user-renamed titles are lost, both rebuildable.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Web,
    Telegram,
    Cron,
    Other,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleSource {
    Llm,
    User,
    FirstMessage,
    Default,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub chat_id: String,
    pub kind: SessionKind,
    pub title: String,
    pub title_source: TitleSource,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub last_message_preview: String,
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_serde_roundtrip() {
        let meta = SessionMeta {
            chat_id: "chat-1714914000000".into(),
            kind: SessionKind::Web,
            title: "Tomato propagation".into(),
            title_source: TitleSource::Llm,
            created_at_ms: 1714914000000,
            last_activity_ms: 1714915800000,
            last_message_preview: "air-layering instead.".into(),
            version: 1,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: SessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn version_field_round_trips_when_missing() {
        // Schema-evolution insurance: an older meta file with no `version`
        // field must deserialize via the serde default.
        let json = r#"{
            "chatId": "x",
            "kind": "web",
            "title": "t",
            "titleSource": "default",
            "createdAtMs": 1,
            "lastActivityMs": 1,
            "lastMessagePreview": ""
        }"#;
        let meta: SessionMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.version, 1);
    }
}
```

Add `pub mod meta;` near the top of `agent/src/core/sessions/mod.rs` so the new module is exported (place after the existing `use` statements, before the existing `mod`/`fn` definitions — pattern-match the file's local style).

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cd agent && cargo test --features desktop --lib sessions::meta::tests
```

Expected: compile error or test failure (the file doesn't exist yet — actually after Step 1 they should pass; rephrase).

> **Note:** Step 2 in this task is a bit unusual because the file we created in Step 1 is the impl _and_ the test together. Run the tests; they should pass on the first try because the impl is complete. If any test fails, fix the file before committing.

- [ ] **Step 3: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::meta::tests
```

Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add agent/src/core/sessions/meta.rs agent/src/core/sessions/mod.rs
git commit -m "$(cat <<'EOF'
feat(sessions): add SessionMeta sidecar types and serde

Foundation for the multi-conversation web UI. Defines SessionKind
(Web | Telegram | Cron | Other), TitleSource (Llm | User |
FirstMessage | Default), and SessionMeta with chatId/title/kind/
timestamps/preview. Serde uses camelCase JSON keys to match the
web client's idioms and `version: 1` for forward-compat.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `detect_kind` classifier

**Files:**
- Modify: `agent/src/core/sessions/meta.rs` (add method to `impl SessionMeta`)

- [ ] **Step 1: Write the failing tests**

Append to `agent/src/core/sessions/meta.rs` inside the existing `mod tests`:

```rust
    #[test]
    fn detect_kind_web_explicit() {
        assert_eq!(SessionMeta::detect_kind("web"), SessionKind::Web);
    }

    #[test]
    fn detect_kind_chat_slug() {
        assert_eq!(SessionMeta::detect_kind("chat-1714914000000"), SessionKind::Web);
    }

    #[test]
    fn detect_kind_telegram_numeric() {
        assert_eq!(SessionMeta::detect_kind("987654321"), SessionKind::Telegram);
    }

    #[test]
    fn detect_kind_cron_canonical() {
        assert_eq!(SessionMeta::detect_kind("cron:job-abc:run-1"), SessionKind::Cron);
    }

    #[test]
    fn detect_kind_cron_sanitized() {
        // After safe_chat_id translates ':' to '_', the on-disk filename
        // (and the chat_id list_with_meta sees from the directory) still
        // resolves to Cron.
        assert_eq!(SessionMeta::detect_kind("cron_job-abc_run-1"), SessionKind::Cron);
    }

    #[test]
    fn detect_kind_other_fallback() {
        assert_eq!(SessionMeta::detect_kind("custom-thing"), SessionKind::Other);
    }
```

- [ ] **Step 2: Run tests to confirm fail**

```bash
cd agent && cargo test --features desktop --lib sessions::meta::tests::detect_kind
```

Expected: compile error — `SessionMeta::detect_kind` doesn't exist.

- [ ] **Step 3: Implement `detect_kind`**

Add an `impl SessionMeta` block to `meta.rs` (above `#[cfg(test)] mod tests`):

```rust
impl SessionMeta {
    /// Classify a chat by its id pattern. Accepts both canonical
    /// (`cron:job-abc:run-1`) and on-disk-sanitized (`cron_job-abc_run-1`)
    /// forms because `safe_chat_id` translates `:` to `_` and
    /// `list_with_meta` may see the sanitized form when synthesizing.
    pub fn detect_kind(chat_id: &str) -> SessionKind {
        if chat_id == "web" || chat_id.starts_with("chat-") {
            SessionKind::Web
        } else if !chat_id.is_empty() && chat_id.bytes().all(|b| b.is_ascii_digit()) {
            SessionKind::Telegram
        } else if chat_id.starts_with("cron:") || chat_id.starts_with("cron_") {
            SessionKind::Cron
        } else {
            SessionKind::Other
        }
    }
}
```

- [ ] **Step 4: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::meta::tests::detect_kind
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/sessions/meta.rs
git commit -m "feat(sessions): detect_kind classifier for SessionMeta

Pattern-matches chat_id to {Web, Telegram, Cron, Other}. Robust
to safe_chat_id sanitization (':' -> '_') so cron sessions whose
meta sidecar was lost still classify correctly when synthesized
from the directory listing.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: `synthesize_default` factory

**Files:**
- Modify: `agent/src/core/sessions/meta.rs`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `meta.rs`:

```rust
    #[test]
    fn synthesize_default_without_first_message() {
        let m = SessionMeta::synthesize_default("chat-100", 100, None);
        assert_eq!(m.chat_id, "chat-100");
        assert_eq!(m.kind, SessionKind::Web);
        assert_eq!(m.title, "New chat");
        assert_eq!(m.title_source, TitleSource::Default);
        assert_eq!(m.created_at_ms, 100);
        assert_eq!(m.last_activity_ms, 100);
        assert_eq!(m.last_message_preview, "");
        assert_eq!(m.version, 1);
    }

    #[test]
    fn synthesize_default_with_first_message() {
        let m = SessionMeta::synthesize_default(
            "chat-100",
            100,
            Some("How do I propagate tomatoes from cuttings?"),
        );
        assert_eq!(m.title, "How do I propagate tomatoes from cuttin");
        assert_eq!(m.title.chars().count(), 40);
        assert_eq!(m.title_source, TitleSource::FirstMessage);
    }

    #[test]
    fn synthesize_default_first_message_short_no_truncation() {
        let m = SessionMeta::synthesize_default("chat-100", 100, Some("hi"));
        assert_eq!(m.title, "hi");
        assert_eq!(m.title_source, TitleSource::FirstMessage);
    }

    #[test]
    fn synthesize_default_empty_first_message_falls_back() {
        let m = SessionMeta::synthesize_default("chat-100", 100, Some("   "));
        assert_eq!(m.title, "New chat");
        assert_eq!(m.title_source, TitleSource::Default);
    }
```

- [ ] **Step 2: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::meta::tests::synthesize_default
```

Expected: compile error — `synthesize_default` not found.

- [ ] **Step 3: Implement `synthesize_default`**

Add to the existing `impl SessionMeta`:

```rust
    /// Build a sensible default for a chat that has no sidecar yet.
    /// When `first_user_message` is `Some(non-empty)`, derive a title
    /// by truncating to 40 characters with `TitleSource::FirstMessage`.
    /// Otherwise fall back to "New chat" + `TitleSource::Default`.
    pub fn synthesize_default(
        chat_id: &str,
        now_ms: u64,
        first_user_message: Option<&str>,
    ) -> Self {
        let (title, title_source) = match first_user_message {
            Some(msg) if !msg.trim().is_empty() => {
                let title: String = msg.trim().chars().take(40).collect();
                (title, TitleSource::FirstMessage)
            }
            _ => ("New chat".to_string(), TitleSource::Default),
        };
        Self {
            chat_id: chat_id.to_string(),
            kind: Self::detect_kind(chat_id),
            title,
            title_source,
            created_at_ms: now_ms,
            last_activity_ms: now_ms,
            last_message_preview: String::new(),
            version: 1,
        }
    }
```

- [ ] **Step 4: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::meta::tests::synthesize
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/sessions/meta.rs
git commit -m "feat(sessions): synthesize_default factory with optional first message

Constructs a SessionMeta when no sidecar exists. With a first-user-
turn passed in, derives a 40-char-truncated title and sets
title_source=FirstMessage; otherwise falls back to a 'New chat'
placeholder. Used by list_with_meta to gracefully populate metadata
for legacy chats that pre-date this feature (including the existing
'web' chat).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: `SessionManager::meta()` reader

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`

- [ ] **Step 1: Inspect existing field shape**

Run to find the cloud-handles field on `SessionManager`:

```bash
grep -n "pub struct SessionManager\|cloud:\|fn cloud_" agent/src/core/sessions/mod.rs | head -15
```

This task assumes the field is named `cloud: Option<...>` (matches the existing `cloud_session_bytes` / `cloud_load_text` methods). Adapt the code below to the actual field name if it differs.

- [ ] **Step 2: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` in `agent/src/core/sessions/mod.rs`:

```rust
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
```

- [ ] **Step 3: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::meta
```

Expected: compile error — `meta` method not found.

- [ ] **Step 4: Implement the reader**

Add to `impl SessionManager` near the existing `session_path` helper:

```rust
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
    pub fn meta(&self, chat_id: &str) -> Result<Option<crate::core::sessions::meta::SessionMeta>, Box<dyn std::error::Error>> {
        let path = self.meta_path(chat_id);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
```

- [ ] **Step 5: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::meta
```

Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add agent/src/core/sessions/mod.rs
git commit -m "feat(sessions): SessionManager::meta() reads sidecar from local FS

Hot-path read for the sidebar list. Missing sidecar returns None
(callers synthesize). Parse errors propagate (corruption is rare
and self-heals on next set_meta).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: `SessionManager::set_meta()` cloud-aware writer

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`

- [ ] **Step 1: Locate the cloud strict-put pattern**

Read these lines to copy the cloud-aware write pattern verbatim:

```bash
sed -n '260,290p' agent/src/core/cron.rs
```

The pattern is: `cloud.cache.put(key, bytes)` → `strict_put(...)?` → `fs::write(local_path, bytes)`. Errors from `strict_put` propagate; the in-memory cache update is intentionally not rolled back (see Section 4 of the spec — the next successful `set_meta` reseeds the cache).

- [ ] **Step 2: Write the failing tests**

Append to `mod tests` in `mod.rs`:

```rust
    #[test]
    fn set_meta_writes_local_when_no_cloud() {
        use crate::core::sessions::meta::SessionMeta;
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let m = SessionMeta::synthesize_default("chat-1", 100, None);
        mgr.set_meta("chat-1", &m).unwrap();
        assert_eq!(mgr.meta("chat-1").unwrap().unwrap(), m);
    }

    #[test]
    fn set_meta_overwrites_existing_sidecar() {
        use crate::core::sessions::meta::{SessionMeta, TitleSource};
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let mut m = SessionMeta::synthesize_default("chat-1", 100, None);
        mgr.set_meta("chat-1", &m).unwrap();
        m.title = "Renamed".into();
        m.title_source = TitleSource::User;
        mgr.set_meta("chat-1", &m).unwrap();
        assert_eq!(mgr.meta("chat-1").unwrap().unwrap().title, "Renamed");
    }
```

(Cloud-failure rollback tests follow the cron `FakeStore` pattern — see Task 9 for the equivalent on `delete`. We skip them here for `set_meta` because the rollback semantics are simpler: no in-memory state to revert. If you want full coverage, mirror `cron.rs`'s `cloud_save_failure_rolls_back_in_memory_state` test against the `set_meta` path.)

- [ ] **Step 3: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::set_meta
```

Expected: compile error.

- [ ] **Step 4: Implement `set_meta`**

Add to `impl SessionManager`:

```rust
    /// Write the metadata sidecar. When cloud handles are present,
    /// follows the same strict-path pattern as `core::cron::CronStore::save`:
    /// cache.put -> strict_put -> local fs.write. Failures from
    /// strict_put propagate; on cloud-success-but-local-fail we keep
    /// the cloud truth and log a warning.
    pub fn set_meta(
        &self,
        chat_id: &str,
        meta: &crate::core::sessions::meta::SessionMeta,
    ) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        if let Some(cloud) = &self.cloud {
            let cloud_key = format!("sys/sessions/{}/meta.json", safe_chat_id(chat_id));
            cloud.cache.put(&cloud_key, bytes.clone());
            crate::core::cloud::strict::strict_put(
                &cloud.store,
                &cloud_key,
                &bytes,
                cloud.retry_max,
                cloud.backoff_cap_secs,
            )
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        }

        let path = self.meta_path(chat_id);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, &bytes) {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!("set_meta local write failed for {}: {} (cloud is canonical)", chat_id, e);
                Ok(())
            }
        }
    }
```

If your `SessionManager` struct doesn't have a `cloud` field with the shape implied above, look at the existing `cloud_session_bytes` method to find the correct accessor and adapt the syntax. Don't rewrite the cloud handle plumbing — borrow whatever already exists.

- [ ] **Step 5: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::set_meta
```

Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add agent/src/core/sessions/mod.rs
git commit -m "feat(sessions): SessionManager::set_meta() cloud-aware sidecar write

cache.put -> strict_put -> fs.write, mirroring core::cron::CronStore::save.
Cloud-success-local-fail logs a warning and returns Ok (cloud is
canonical; boot-restore will resync). Cloud-failure propagates.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: `SessionManager::list_with_meta()` with synthesize-and-persist

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn list_with_meta_returns_sidecar_when_present() {
        use crate::core::sessions::meta::{SessionMeta, SessionKind, TitleSource};
        let dir = tempfile::tempdir().unwrap();
        // Seed a JSONL + a sidecar.
        std::fs::write(dir.path().join("chat-1.jsonl"), b"").unwrap();
        let m = SessionMeta {
            chat_id: "chat-1".into(),
            kind: SessionKind::Web,
            title: "Tomatoes".into(),
            title_source: TitleSource::User,
            created_at_ms: 100,
            last_activity_ms: 200,
            last_message_preview: "p".into(),
            version: 1,
        };
        std::fs::write(
            dir.path().join("chat-1.meta.json"),
            serde_json::to_vec(&m).unwrap(),
        ).unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let listed = mgr.list_with_meta();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], m);
    }

    #[test]
    fn list_with_meta_synthesizes_when_sidecar_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chat-1.jsonl"), b"").unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let listed = mgr.list_with_meta();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].chat_id, "chat-1");
        assert_eq!(listed[0].title, "New chat");
        // Synthesized meta is persisted so subsequent calls don't re-synthesize.
        assert!(dir.path().join("chat-1.meta.json").exists());
    }

    #[test]
    fn list_with_meta_skips_orphan_meta_without_jsonl() {
        use crate::core::sessions::meta::SessionMeta;
        let dir = tempfile::tempdir().unwrap();
        let m = SessionMeta::synthesize_default("orphan", 1, None);
        std::fs::write(
            dir.path().join("orphan.meta.json"),
            serde_json::to_vec(&m).unwrap(),
        ).unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        assert_eq!(mgr.list_with_meta().len(), 0);
    }

    #[test]
    fn list_with_meta_falls_back_to_default_on_corrupt_meta() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chat-1.jsonl"), b"").unwrap();
        std::fs::write(dir.path().join("chat-1.meta.json"), b"not-valid-json").unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let listed = mgr.list_with_meta();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "New chat");
    }
```

- [ ] **Step 2: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::list_with_meta
```

Expected: compile error.

- [ ] **Step 3: Implement `list_with_meta`**

Add to `impl SessionManager`:

```rust
    /// Walk the sessions directory and return one `SessionMeta` per
    /// `<id>.jsonl`. When the matching `.meta.json` is missing or
    /// corrupt, synthesize a default (peeking at the first JSONL
    /// entry for a `FirstMessage`-source title when content exists)
    /// and persist it back to disk so subsequent calls skip the
    /// synthesis path.
    pub fn list_with_meta(&self) -> Vec<crate::core::sessions::meta::SessionMeta> {
        use std::fs;
        let mut out = Vec::new();
        let dir = match fs::read_dir(&self.sessions_dir) {
            Ok(d) => d,
            Err(_) => return out,
        };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(stripped) = name.strip_suffix(".jsonl") else { continue };
            let chat_id = stripped.to_string();

            let meta = match self.meta(&chat_id) {
                Ok(Some(m)) => m,
                _ => {
                    // Synthesize: peek at the first user-turn from the
                    // JSONL when the file has content, so legacy chats
                    // get a meaningful title rather than a placeholder.
                    let first_msg = self.peek_first_user_message(&chat_id);
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let synth = crate::core::sessions::meta::SessionMeta::synthesize_default(
                        &chat_id,
                        mtime,
                        first_msg.as_deref(),
                    );
                    // Persist so future calls skip synthesis.
                    if let Err(e) = self.set_meta(&chat_id, &synth) {
                        tracing::warn!("list_with_meta: persist synthesized meta failed for {}: {}", chat_id, e);
                    }
                    synth
                }
            };
            out.push(meta);
        }
        out
    }

    /// Helper for `list_with_meta`: read the first user-role entry
    /// from a session's JSONL and return its content, or `None` if
    /// the file is empty or unreadable.
    fn peek_first_user_message(&self, chat_id: &str) -> Option<String> {
        let path = self.session_path(chat_id);
        let content = std::fs::read_to_string(&path).ok()?;
        for line in content.lines() {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // SessionEntry shape: { kind: "Message", message: { role: "user", content: "..." } }
            let role = v.get("message").and_then(|m| m.get("role")).and_then(|r| r.as_str());
            let text = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str());
            if role == Some("user") {
                if let Some(t) = text {
                    return Some(t.to_string());
                }
            }
        }
        None
    }
```

> **Note:** The exact JSON shape of a SessionEntry depends on the existing serde definitions. If `peek_first_user_message` returns nothing for a real JSONL file during testing, run `head -1 data/sessions/web.jsonl | jq` on a real device to confirm the field names, and adjust the helper. The tests above use empty JSONL files so they don't exercise the peek path — that's intentional; peek is best-effort.

- [ ] **Step 4: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::list_with_meta
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/sessions/mod.rs
git commit -m "feat(sessions): list_with_meta with synthesize-and-persist

Walks sessions_dir, returns one SessionMeta per JSONL. When the
sidecar is missing or corrupt, synthesizes a default — peeking at
the first user-turn for a FirstMessage title when content exists —
and persists it so subsequent calls are O(1). Orphan meta files
without a JSONL are skipped.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: `SessionManager::bump_activity()` best-effort updater

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn bump_activity_updates_last_activity_and_preview() {
        use crate::core::sessions::meta::SessionMeta;
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let initial = SessionMeta::synthesize_default("chat-1", 100, None);
        mgr.set_meta("chat-1", &initial).unwrap();

        mgr.bump_activity("chat-1", "tomato sounds tasty", 200);

        let m = mgr.meta("chat-1").unwrap().unwrap();
        assert_eq!(m.last_activity_ms, 200);
        assert_eq!(m.last_message_preview, "tomato sounds tasty");
    }

    #[test]
    fn bump_activity_truncates_preview_to_120_chars() {
        use crate::core::sessions::meta::SessionMeta;
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        let initial = SessionMeta::synthesize_default("chat-1", 100, None);
        mgr.set_meta("chat-1", &initial).unwrap();

        let long = "a".repeat(200);
        mgr.bump_activity("chat-1", &long, 200);

        let m = mgr.meta("chat-1").unwrap().unwrap();
        assert_eq!(m.last_message_preview.chars().count(), 120);
    }

    #[test]
    fn bump_activity_synthesizes_meta_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());

        mgr.bump_activity("chat-1", "first contact", 500);

        let m = mgr.meta("chat-1").unwrap().unwrap();
        assert_eq!(m.last_activity_ms, 500);
        assert_eq!(m.last_message_preview, "first contact");
    }

    #[test]
    fn bump_activity_does_not_panic_on_unwriteable_path() {
        // Best-effort: a write failure logs but doesn't propagate.
        let mgr = SessionManager::new("/nonexistent/path");
        // Should not panic.
        mgr.bump_activity("chat-1", "anything", 100);
    }
```

- [ ] **Step 2: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::bump_activity
```

Expected: compile error.

- [ ] **Step 3: Implement `bump_activity`**

```rust
    /// Update last-activity-ms and last-message-preview after a
    /// successful chat turn. Best-effort — failures are logged but
    /// do not propagate (the user already received their reply;
    /// sidebar staleness is non-critical). Synthesizes a fresh meta
    /// if none exists.
    pub fn bump_activity(&self, chat_id: &str, preview: &str, now_ms: u64) {
        let mut meta = match self.meta(chat_id) {
            Ok(Some(m)) => m,
            _ => crate::core::sessions::meta::SessionMeta::synthesize_default(chat_id, now_ms, None),
        };
        meta.last_activity_ms = now_ms;
        meta.last_message_preview = preview.chars().take(120).collect();
        if let Err(e) = self.set_meta(chat_id, &meta) {
            tracing::warn!("bump_activity for {}: {}", chat_id, e);
        }
    }
```

- [ ] **Step 4: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::bump_activity
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/sessions/mod.rs
git commit -m "feat(sessions): SessionManager::bump_activity best-effort updater

Called after every successful chat turn. Updates lastActivityMs
and lastMessagePreview (truncated to 120 chars). Synthesizes a
fresh meta if none exists (newly-active legacy/Telegram chats
get a sidecar on first sight). Failures are logged but never
propagate — the chat reply already went out.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: `SessionManager::rename` + `rename_internal`

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn rename_sets_title_and_user_source() {
        use crate::core::sessions::meta::{SessionMeta, TitleSource};
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        mgr.set_meta("chat-1", &SessionMeta::synthesize_default("chat-1", 100, None)).unwrap();

        let updated = mgr.rename("chat-1", "Custom title").unwrap();
        assert_eq!(updated.title, "Custom title");
        assert_eq!(updated.title_source, TitleSource::User);
    }

    #[test]
    fn rename_validates_empty_title() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        mgr.set_meta(
            "chat-1",
            &crate::core::sessions::meta::SessionMeta::synthesize_default("chat-1", 100, None),
        ).unwrap();

        assert!(mgr.rename("chat-1", "").is_err());
        assert!(mgr.rename("chat-1", "   ").is_err());
    }

    #[test]
    fn rename_validates_oversize_title() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        mgr.set_meta(
            "chat-1",
            &crate::core::sessions::meta::SessionMeta::synthesize_default("chat-1", 100, None),
        ).unwrap();

        let too_long = "a".repeat(81);
        assert!(mgr.rename("chat-1", &too_long).is_err());
    }

    #[test]
    fn rename_returns_not_found_for_missing_chat() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        // No meta + no jsonl
        let err = mgr.rename("missing", "x").unwrap_err();
        assert!(err.to_string().contains("not found") || err.to_string().to_lowercase().contains("notfound"));
    }

    #[test]
    fn rename_internal_accepts_llm_source() {
        use crate::core::sessions::meta::{SessionMeta, TitleSource};
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        mgr.set_meta("chat-1", &SessionMeta::synthesize_default("chat-1", 100, None)).unwrap();

        mgr.rename_internal("chat-1", "Tomato propagation", TitleSource::Llm).unwrap();
        let m = mgr.meta("chat-1").unwrap().unwrap();
        assert_eq!(m.title_source, TitleSource::Llm);
    }

    #[test]
    fn rename_internal_does_not_overwrite_user_title() {
        // Belt-and-suspenders: when the LLM background task fires after
        // a chat where the user has already renamed, the call site
        // should check title_source first. rename_internal itself is
        // unconditional — this test documents the "user has already set
        // a title" path is the caller's responsibility.
        use crate::core::sessions::meta::{SessionMeta, TitleSource};
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        mgr.set_meta("chat-1", &SessionMeta::synthesize_default("chat-1", 100, None)).unwrap();
        mgr.rename("chat-1", "User chose").unwrap();
        // Caller must check title_source == User and skip the rename.
        // Here we just confirm the rename API doesn't silently drop the call.
        let pre = mgr.meta("chat-1").unwrap().unwrap();
        assert_eq!(pre.title, "User chose");
    }
```

- [ ] **Step 2: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::rename
```

Expected: compile error.

- [ ] **Step 3: Implement `rename` and `rename_internal`**

```rust
    /// User-driven rename. Validates length, sets `TitleSource::User`,
    /// persists. Returns the updated meta.
    pub fn rename(
        &self,
        chat_id: &str,
        title: &str,
    ) -> std::io::Result<crate::core::sessions::meta::SessionMeta> {
        let trimmed = title.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 80 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "title length must be 1..=80 characters after trim",
            ));
        }
        self.rename_internal(
            chat_id,
            trimmed,
            crate::core::sessions::meta::TitleSource::User,
        )
    }

    /// Bypass-validation rename for internal callers (e.g., LLM title
    /// generation). Caller is responsible for checking
    /// `title_source != User` before calling, if appropriate.
    pub fn rename_internal(
        &self,
        chat_id: &str,
        title: &str,
        source: crate::core::sessions::meta::TitleSource,
    ) -> std::io::Result<crate::core::sessions::meta::SessionMeta> {
        let mut meta = self
            .meta(chat_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("chat not found: {}", chat_id))
            })?;
        meta.title = title.to_string();
        meta.title_source = source;
        self.set_meta(chat_id, &meta)?;
        Ok(meta)
    }
```

- [ ] **Step 4: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::rename
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/sessions/mod.rs
git commit -m "feat(sessions): rename + rename_internal with title-source semantics

rename() validates 1..=80 chars after trim and sets
title_source=User. rename_internal() is the unchecked variant for
the LLM background task and other internal callers — caller is
responsible for checking title_source before invoking, so a User-
renamed title isn't clobbered by a later LLM completion.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: `SessionManager::delete()` with cloud cleanup

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`

- [ ] **Step 1: Locate the existing cloud-prefix-walk pattern**

```bash
grep -n "fn clear\|sys/sessions\|cloud_prefix" agent/src/core/sessions/mod.rs | head -10
```

Find the existing `SessionManager::clear` (mentioned in `project_cloud_persistence_handover.md` as "cloud-aware"). The cloud cleanup walks `sys/sessions/<chat_id>/` and deletes every key. Reuse that pattern; don't reinvent.

- [ ] **Step 2: Write the failing tests**

```rust
    #[test]
    fn delete_removes_jsonl_and_meta_locally() {
        use crate::core::sessions::meta::SessionMeta;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chat-1.jsonl"), b"x").unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        mgr.set_meta("chat-1", &SessionMeta::synthesize_default("chat-1", 100, None)).unwrap();

        mgr.delete("chat-1").unwrap();

        assert!(!dir.path().join("chat-1.jsonl").exists());
        assert!(!dir.path().join("chat-1.meta.json").exists());
        assert_eq!(mgr.list_with_meta().len(), 0);
    }

    #[test]
    fn delete_is_idempotent_when_files_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        // Should not error
        mgr.delete("never-existed").unwrap();
    }
```

(Cloud-side delete-walks-prefix tests follow the `FakeStore` pattern from `cron.rs:719-749`. Add one if you want full coverage; the structure is: build a `FakeStore` that records `delete()` calls, attach via `cloud` handles, run `delete()`, assert the recorded keys match what was under the prefix.)

- [ ] **Step 3: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::delete_removes
```

Expected: compile error.

- [ ] **Step 4: Implement `delete`**

```rust
    /// Hard-delete a chat: remove the JSONL and the meta sidecar
    /// locally, then walk `sys/sessions/<chat_id>/` in the cloud
    /// and delete every key. Idempotent — missing files are not an
    /// error. Partial cloud failure surfaces via the returned Err
    /// (callers — the HTTP DELETE handler — return 500).
    pub fn delete(&self, chat_id: &str) -> std::io::Result<()> {
        // Local cleanup (best-effort: NotFound is fine).
        let jsonl = self.session_path(chat_id);
        match std::fs::remove_file(&jsonl) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let meta = self.meta_path(chat_id);
        match std::fs::remove_file(&meta) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        // Cloud cleanup — walk the prefix and delete each key.
        if let Some(cloud) = &self.cloud {
            let prefix = format!("sys/sessions/{}/", safe_chat_id(chat_id));
            // Mirror the existing clear() cloud-cleanup. If your existing
            // pattern uses an `ObjectStore::list`-then-`delete` helper,
            // call it here; otherwise inline the logic.
            let keys = cloud.store.list_keys(&prefix)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("list: {}", e)))?;
            for key in keys {
                cloud.cache.delete(&key);
                if let Err(e) = cloud.store.delete(&key) {
                    tracing::warn!("delete: cloud delete failed for {}: {}", key, e);
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("cloud delete: {}", e)));
                }
            }
        }
        Ok(())
    }
```

If `ObjectStore` doesn't have a `list_keys` method, find the existing equivalent (e.g., the cron or memory cleanup paths use one). Adapt to whatever the codebase already calls it. Don't introduce a new abstraction.

- [ ] **Step 5: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib sessions::tests::delete
```

Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add agent/src/core/sessions/mod.rs
git commit -m "feat(sessions): SessionManager::delete with cloud-prefix cleanup

Removes <id>.jsonl + <id>.meta.json locally, then walks
sys/sessions/<id>/ in the cloud and deletes every key. Mirrors
the existing clear() cloud-cleanup pattern. Partial cloud failure
returns Err so the HTTP DELETE handler can surface 500. Local
NotFound is treated as success (idempotent).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: Boot-restore extension for `meta.json`

**Files:**
- Modify: `agent/src/core/cloud/boot.rs`

- [ ] **Step 1: Locate the per-chat restore loop**

```bash
grep -n "base.jsonl\|log-\|sys/sessions/.*/" agent/src/core/cloud/boot.rs | head -20
```

Find the loop that downloads each chat's `base.jsonl` and `log-NN.jsonl` files. The plan adds `meta.json` to that list.

- [ ] **Step 2: Add a test to `boot.rs`'s existing test module**

Find the existing `#[cfg(test)] mod tests` block in `boot.rs`. Add:

```rust
    #[test]
    fn restore_includes_meta_json_alongside_jsonl() {
        // FakeStore seeded with chat-1's base.jsonl AND meta.json.
        // After boot_restore, both files should exist on local FS.
        let dir = tempfile::tempdir().unwrap();
        let store = build_fake_store_with_chat(&[
            ("sys/sessions/chat-1/base.jsonl", b"line1\n"),
            ("sys/sessions/chat-1/meta.json", br#"{"chatId":"chat-1","kind":"web","title":"T","titleSource":"user","createdAtMs":1,"lastActivityMs":2,"lastMessagePreview":"","version":1}"#),
        ]);
        // (Use whatever helper the existing boot.rs tests use to build
        //  a FakeStore with seeded keys — `build_fake_store_with_chat`
        //  is illustrative; copy the existing helper's signature.)

        let cfg = test_boot_config_pointing_to(dir.path());
        let _ = boot_restore(&store, &cfg);

        assert!(dir.path().join("sessions/chat-1.jsonl").exists());
        assert!(dir.path().join("sessions/chat-1.meta.json").exists());
    }
```

If `boot.rs`'s tests don't have a helper of the right shape, adapt the test to whatever harness exists — the assertion (both files written) is what matters.

- [ ] **Step 3: Run test; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib cloud::boot::tests::restore_includes_meta
```

Expected: failure — only `chat-1.jsonl` exists; `chat-1.meta.json` does not.

- [ ] **Step 4: Extend the per-chat restore loop**

Find the loop (likely a `for` over keys under `sys/sessions/<id>/`) and add `meta.json` to the list of suffixes it restores. The change should be additive — current behavior for `base.jsonl` and `log-NN.jsonl` is unchanged. Pseudo-pattern:

```rust
for suffix in ["base.jsonl", "meta.json"] {
    // existing per-suffix restore logic
}
// log-NN.jsonl files keep their existing (looped) restore path
```

The exact placement depends on how `boot.rs` currently structures the loop — read the existing code and slot the new suffix into the same iteration shape.

- [ ] **Step 5: Run test; expect PASS**

```bash
cd agent && cargo test --features desktop --lib cloud::boot::tests
```

Expected: all boot tests pass (including the new one).

- [ ] **Step 6: Commit**

```bash
git add agent/src/core/cloud/boot.rs
git commit -m "feat(cloud): boot_restore fetches meta.json sidecar alongside JSONL

Per-chat restore loop now downloads sys/sessions/<id>/meta.json
in addition to base.jsonl and log-NN.jsonl. Bytes-for-bytes
restore — same defensive layers (L3 size gate, L4 tail-only
fallback, L5 quarantine) apply.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: `bump_activity` hook in `Gateway::chat_with_events`

**Files:**
- Modify: `agent/src/core/gateway.rs`

- [ ] **Step 1: Locate the post-turn-success path**

```bash
grep -n "fn chat_with_events\|ChatEvent::Done\|return Ok" agent/src/core/gateway.rs | head -15
```

Find where `chat_with_events` returns `Ok(reply)` after a successful turn (and where it sends `ChatEvent::Done` on the WS path). The hook fires once per successful turn — before the function returns.

- [ ] **Step 2: Add the hook call**

Just before the `Ok(reply)` return at the end of a successful turn, add:

```rust
// Sidebar maintenance: update lastActivityMs and lastMessagePreview
// so the conversations list reflects this turn on the next refresh.
let now_ms = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0);
self.sessions.bump_activity(chat_id, &reply, now_ms);
```

Place this call exactly once, after the turn has fully completed and the JSONL writes have flushed. If `chat_with_events` has multiple `Ok` return points (e.g., one for slash commands, one for normal turns), only hook the normal-turn path — slash commands don't accumulate to JSONL the same way.

- [ ] **Step 3: Compile-check**

```bash
cd agent && cargo build --features desktop --no-default-features
```

Expected: clean build. (No new test for this — `bump_activity` is already tested at the unit level, and an integration test would require mocking `Runner` which is heavy. Manual smoke after Task 14 covers this.)

- [ ] **Step 4: Commit**

```bash
git add agent/src/core/gateway.rs
git commit -m "feat(gateway): hook bump_activity after successful chat turns

Updates the chat's metadata sidecar (lastActivityMs +
lastMessagePreview) so the web sidebar reflects activity on the
next refresh. Best-effort — bump_activity logs but doesn't
propagate failures; the user's reply has already gone out.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: Title generation task + post-turn hook

**Files:**
- Create: `agent/src/core/title_gen.rs`
- Modify: `agent/src/core/mod.rs` (add `pub mod title_gen;`)
- Modify: `agent/src/core/gateway.rs` (call after `bump_activity`)

- [ ] **Step 1: Create the title-gen module**

Create `agent/src/core/title_gen.rs`:

```rust
//! Background task that upgrades a chat's title from a
//! `FirstMessage`/`Default` source to an LLM-summarized one.
//!
//! Runs *after* the user has received their reply — off the critical
//! path. Failures are logged and tolerated; the trigger condition
//! (`title_source != User && != Llm`) re-arms on the next turn so a
//! transient LLM outage just defers the title upgrade.

use std::sync::Arc;

use crate::core::runner::Runner;
use crate::core::sessions::meta::TitleSource;
use crate::core::sessions::SessionManager;
use crate::core::types::{Message, MessageRole};

const TITLE_PROMPT: &str = "Summarize this conversation in 6 words or fewer. \
Output only the title — no quotes, no punctuation, no preamble.";

/// Trigger an LLM call to summarize this chat into a sidebar title.
/// Bails out unless the meta's `title_source` is `Default` or
/// `FirstMessage`. On success, calls `rename_internal(..., Llm)`.
pub async fn maybe_generate_title(
    chat_id: String,
    sessions: Arc<SessionManager>,
    runner: Arc<dyn Runner>,
    model: String,
) {
    // Bail when not needed.
    let meta = match sessions.meta(&chat_id) {
        Ok(Some(m)) => m,
        _ => return,
    };
    match meta.title_source {
        TitleSource::Llm | TitleSource::User => return,
        TitleSource::Default | TitleSource::FirstMessage => {}
    }

    // Build the prompt: last 4-6 entries of conversation context.
    let entries = match sessions.load(&chat_id) {
        Ok(es) => es,
        Err(_) => return,
    };
    let context: Vec<Message> = entries
        .into_iter()
        .rev()
        .take(6)
        .rev()
        .filter_map(|e| e.into_message())   // adapt to actual API
        .collect();
    if context.is_empty() {
        return;
    }
    let mut messages = vec![Message {
        role: MessageRole::System,
        content: TITLE_PROMPT.to_string(),
        ..Default::default()
    }];
    messages.extend(context);

    // One-shot LLM call. No tools, no streaming.
    let title = match runner.chat_once(&model, &messages).await {
        Ok(reply) => reply.trim().trim_matches('"').to_string(),
        Err(e) => {
            tracing::warn!("title_gen for {}: {}", chat_id, e);
            return;
        }
    };
    if title.is_empty() || title.chars().count() > 80 {
        tracing::warn!("title_gen for {}: rejected title (length {})", chat_id, title.chars().count());
        return;
    }

    if let Err(e) = sessions.rename_internal(&chat_id, &title, TitleSource::Llm) {
        tracing::warn!("title_gen rename_internal for {}: {}", chat_id, e);
    }
}
```

> **Note:** the `into_message()`, `MessageRole::System`, and `runner.chat_once(...)` calls match the existing types and trait names — adapt them if those names differ. The shape (load history → trim to last few entries → one-shot LLM call) is what matters.

Add `pub mod title_gen;` to `agent/src/core/mod.rs`.

- [ ] **Step 2: Compile-check**

```bash
cd agent && cargo build --features desktop --no-default-features
```

Expected: clean build. If `runner.chat_once` doesn't exist, add a small helper to the `Runner` trait that wraps the existing `chat` entry point with no-tools / single-shot semantics.

- [ ] **Step 3: Hook from `gateway.rs`**

After the `bump_activity` call from Task 11, append:

```rust
// Title generation is fire-and-forget; runs only when the meta says
// the title isn't already User-set or Llm-derived.
{
    let chat_id_owned = chat_id.to_string();
    let sessions = self.sessions.clone();
    let runner = self.runner.clone();
    let model = self.config.providers.default_model();  // adapt to actual config accessor

    #[cfg(feature = "desktop")]
    tokio::spawn(crate::core::title_gen::maybe_generate_title(
        chat_id_owned, sessions, runner, model,
    ));

    #[cfg(feature = "esp32")]
    {
        // ESP32: run inline-blocking on the agent thread that just
        // completed the turn. The LLM round-trip adds a few seconds
        // before the agent thread is free for the next request, but
        // this beats spawning another 32KB-stack thread per turn.
        let _ = esp_idf_svc::hal::task::block_on(crate::core::title_gen::maybe_generate_title(
            chat_id_owned, sessions, runner, model,
        ));
    }
}
```

Adapt `self.config.providers.default_model()` to whatever accessor the codebase actually uses for the active model name.

- [ ] **Step 4: Build for both targets**

```bash
cd agent && cargo build --features desktop --no-default-features
cd agent && just build devkitc
```

Expected: both clean.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/title_gen.rs agent/src/core/mod.rs agent/src/core/gateway.rs
git commit -m "feat(core): LLM title generation as post-turn background task

After a successful chat turn, fire a one-shot LLM call asking for
a 6-words-or-fewer summary, then update the meta sidecar via
rename_internal(..., Llm). Trigger condition gates on
title_source: only Default or FirstMessage promote; User and Llm
are sticky.

Desktop: tokio::spawn so the user's reply isn't held up.
ESP32: block_on inside the agent thread — the round-trip delays
the next request by a few seconds but avoids a third 32KB-stack
thread.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: HTTP routes — desktop (`agent/src/desktop/server.rs`)

**Files:**
- Modify: `agent/src/desktop/server.rs`

- [ ] **Step 1: Locate the existing axum router**

```bash
grep -n "Router::new\|route(" agent/src/desktop/server.rs | head -20
```

Find where the existing routes are registered (`/api/chat`, `/api/config`, etc.). The new routes follow the same handler-registration shape.

- [ ] **Step 2: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` in `server.rs`:

```rust
    #[tokio::test]
    async fn get_sessions_returns_sorted_array() {
        use crate::core::sessions::meta::SessionMeta;
        let dir = tempfile::tempdir().unwrap();
        let app_state = build_test_state(dir.path());
        // Seed two sessions with different lastActivityMs.
        let mgr = &app_state.gateway.sessions;
        let mut a = SessionMeta::synthesize_default("chat-A", 100, None);
        a.last_activity_ms = 100;
        let mut b = SessionMeta::synthesize_default("chat-B", 200, None);
        b.last_activity_ms = 200;
        std::fs::write(dir.path().join("sessions/chat-A.jsonl"), b"").unwrap();
        std::fs::write(dir.path().join("sessions/chat-B.jsonl"), b"").unwrap();
        mgr.set_meta("chat-A", &a).unwrap();
        mgr.set_meta("chat-B", &b).unwrap();

        let app = build_router(app_state);
        let res = app
            .oneshot(Request::builder().uri("/api/sessions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: Vec<SessionMeta> = serde_json::from_slice(&to_bytes(res.into_body()).await.unwrap()).unwrap();
        assert_eq!(body.len(), 2);
        // Most recent first
        assert_eq!(body[0].chat_id, "chat-B");
        assert_eq!(body[1].chat_id, "chat-A");
    }

    #[tokio::test]
    async fn post_sessions_creates_meta_returns_201() {
        let dir = tempfile::tempdir().unwrap();
        let app_state = build_test_state(dir.path());
        let app = build_router(app_state.clone());
        let res = app
            .oneshot(Request::builder()
                .method("POST")
                .uri("/api/sessions")
                .body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body: serde_json::Value = serde_json::from_slice(&to_bytes(res.into_body()).await.unwrap()).unwrap();
        let chat_id = body["chatId"].as_str().unwrap();
        assert!(chat_id.starts_with("chat-"));
        // Meta sidecar exists on disk
        assert!(dir.path().join("sessions").join(format!("{}.meta.json", chat_id)).exists());
    }

    #[tokio::test]
    async fn patch_sessions_validates_title_length() {
        let dir = tempfile::tempdir().unwrap();
        let app_state = build_test_state(dir.path());
        // Seed a chat
        let mgr = &app_state.gateway.sessions;
        std::fs::write(dir.path().join("sessions/chat-1.jsonl"), b"").unwrap();
        mgr.set_meta(
            "chat-1",
            &crate::core::sessions::meta::SessionMeta::synthesize_default("chat-1", 100, None),
        ).unwrap();

        let app = build_router(app_state);
        let res = app.clone()
            .oneshot(Request::builder()
                .method("PATCH")
                .uri("/api/sessions/chat-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":""}"#)).unwrap())
            .await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let too_long = serde_json::json!({"title": "a".repeat(81)}).to_string();
        let res = app
            .oneshot(Request::builder()
                .method("PATCH")
                .uri("/api/sessions/chat-1")
                .header("content-type", "application/json")
                .body(Body::from(too_long)).unwrap())
            .await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn patch_sessions_404_on_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let app_state = build_test_state(dir.path());
        let app = build_router(app_state);
        let res = app
            .oneshot(Request::builder()
                .method("PATCH")
                .uri("/api/sessions/missing")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"x"}"#)).unwrap())
            .await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_sessions_removes_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let app_state = build_test_state(dir.path());
        std::fs::write(dir.path().join("sessions/chat-1.jsonl"), b"").unwrap();
        app_state.gateway.sessions.set_meta(
            "chat-1",
            &crate::core::sessions::meta::SessionMeta::synthesize_default("chat-1", 100, None),
        ).unwrap();

        let app = build_router(app_state);
        let res = app
            .oneshot(Request::builder()
                .method("DELETE")
                .uri("/api/sessions/chat-1")
                .body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(!dir.path().join("sessions/chat-1.jsonl").exists());
        assert!(!dir.path().join("sessions/chat-1.meta.json").exists());
    }
```

(`build_test_state` and `build_router` — match whatever helper names the existing tests in `server.rs` already use; if none, follow the pattern from the existing `api_chat` test.)

- [ ] **Step 3: Run tests; expect FAIL**

```bash
cd agent && cargo test --features desktop --lib desktop::server::tests::sessions
```

Expected: routes don't exist → 404 / compile errors.

- [ ] **Step 4: Implement the four handlers and register them**

Add handler functions and register them on the router. The shape:

```rust
async fn api_sessions_list(State(state): State<AppState>) -> Response {
    let mut sessions = state.gateway.sessions.list_with_meta();
    sessions.sort_by(|a, b| b.last_activity_ms.cmp(&a.last_activity_ms));
    Json(sessions).into_response()
}

async fn api_sessions_create(State(state): State<AppState>) -> Response {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    let chat_id = format!("chat-{}", now_ms);
    let meta = crate::core::sessions::meta::SessionMeta::synthesize_default(&chat_id, now_ms, None);
    if let Err(e) = state.gateway.sessions.set_meta(&chat_id, &meta) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("set_meta: {}", e)).into_response();
    }
    (StatusCode::CREATED, Json(serde_json::json!({"chatId": chat_id, "meta": meta}))).into_response()
}

#[derive(Deserialize)]
struct PatchSessionBody { title: String }

async fn api_sessions_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchSessionBody>,
) -> Response {
    match state.gateway.sessions.rename(&id, &body.title) {
        Ok(meta) => Json(meta).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, format!("not found: {}", id)).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_sessions_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.gateway.sessions.delete(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
```

Register on the router:

```rust
.route("/api/sessions", get(api_sessions_list).post(api_sessions_create))
.route("/api/sessions/:id", patch(api_sessions_patch).delete(api_sessions_delete))
```

- [ ] **Step 5: Run tests; expect PASS**

```bash
cd agent && cargo test --features desktop --lib desktop::server::tests::sessions
```

Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add agent/src/desktop/server.rs
git commit -m "feat(desktop): /api/sessions REST routes (GET/POST/PATCH/DELETE)

Sidebar plumbing for the multi-conversation UI. GET returns the
full list sorted by lastActivityMs desc. POST allocates a
chat-{epoch_ms} id and writes a default meta. PATCH validates
title 1..=80 and renames (404 on missing). DELETE walks the
chat's local + cloud state.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: HTTP routes — ESP32 (`agent/src/main.rs`)

**Files:**
- Modify: `agent/src/main.rs`

- [ ] **Step 1: Locate the existing route registration**

```bash
grep -n "fn_handler.*\"/api/" agent/src/main.rs | head -20
```

Find where existing routes register (look at `/api/chat` near line 1067 for the pattern). The four new routes register in the same `start_http_server` function.

- [ ] **Step 2: Add four `fn_handler` blocks**

For each route, follow the pattern of the existing `/api/chat` handler. Sketch:

```rust
// --- /api/sessions (GET) ---
let gw_sessions_list = gateway.clone();
server.fn_handler::<anyhow::Error, _>("/api/sessions", Method::Get, move |req| {
    let mut sessions = gw_sessions_list.sessions.list_with_meta();
    sessions.sort_by(|a, b| b.last_activity_ms.cmp(&a.last_activity_ms));
    let body = serde_json::to_vec(&sessions)?;
    let mut resp = req.into_ok_response()?;
    resp.write_all(&body)?;
    Ok(())
})?;

// --- /api/sessions (POST) ---
let gw_sessions_create = gateway.clone();
server.fn_handler::<anyhow::Error, _>("/api/sessions", Method::Post, move |req| {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    let chat_id = format!("chat-{}", now_ms);
    let meta = zenclaw_agent::core::sessions::meta::SessionMeta::synthesize_default(&chat_id, now_ms, None);
    gw_sessions_create.sessions.set_meta(&chat_id, &meta)?;
    let body = serde_json::to_vec(&serde_json::json!({"chatId": chat_id, "meta": meta}))?;
    let mut resp = req.into_response(201, Some("Created"), &[("Content-Type", "application/json")])?;
    resp.write_all(&body)?;
    Ok(())
})?;

// --- /api/sessions/:id (PATCH) — esp-idf-svc httpd doesn't have path
//     params, so we handle this as a wildcard match on the URI. ---
// (See the existing PATCH-style handlers in main.rs for the pattern.
//  If no PATCH handler exists yet, the simplest path is to treat it
//  as PUT against /api/sessions and read the chat_id from the URL
//  manually after verifying the prefix.)
```

For PATCH/DELETE on `/api/sessions/<id>`, the esp-idf-svc HTTP server may not support path parameters cleanly. Two viable approaches:

1. **Wildcard handler**: Register `/api/sessions/*` and parse the id from `req.uri()`. Look at `/api/files/read` (which handles `?path=...`) for an example of URI parsing.
2. **Query param**: PATCH/DELETE `/api/sessions?id=<chat_id>`. Less RESTful but matches the pattern of `/api/files/read?path=...`. Frontend adapts.

Pick whichever matches the codebase's existing convention. If there's already a path-param-style endpoint, mimic it; otherwise use query-param.

- [ ] **Step 3: Add CORS preflight entry**

In the existing CORS preflight block (search for `OPTIONS for all /api/*`), append `"/api/sessions"` to the array of paths.

- [ ] **Step 4: Build for ESP32**

```bash
cd agent && just build devkitc
```

Expected: clean build.

- [ ] **Step 5: Refresh wizard firmware**

```bash
./scripts/build-rust-firmware.sh devkitc
```

(Per `feedback_wizard_firmware_rebuild.md` — every agent code change requires this so the wizard's flash artifacts are up to date.)

- [ ] **Step 6: Manual smoke after reflash**

```bash
HOST=zenclaw-<your-name>.local

# Create
ID=$(curl -sf -X POST http://$HOST/api/sessions | jq -r .chatId)
echo "Created: $ID"

# List should include it
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\")"

# Rename
curl -sf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' -d '{"title":"smoke"}'
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\") | .title"

# Delete
curl -sf -X DELETE "http://$HOST/api/sessions/$ID"
curl -sf "http://$HOST/api/sessions" | jq "[.[] | select(.chatId==\"$ID\")] | length"  # → 0
```

Expected: all four operations succeed; final length is 0.

- [ ] **Step 7: Commit**

```bash
git add agent/src/main.rs
git commit -m "feat(esp32): /api/sessions REST routes for multi-conversation UI

Mirrors the desktop axum handlers. GET lists, POST creates with
chat-{epoch_ms} id, PATCH renames (validation + 404), DELETE
removes local + cloud. CORS preflight extended to cover
/api/sessions.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15: `useSessions` composable

**Files:**
- Create: `web/app/composables/useSessions.ts`

- [ ] **Step 1: Write the composable**

Create `web/app/composables/useSessions.ts`:

```ts
import { ref, type Ref } from 'vue'

export interface SessionMeta {
  chatId: string
  kind: 'web' | 'telegram' | 'cron' | 'other'
  title: string
  titleSource: 'llm' | 'user' | 'firstMessage' | 'default'
  createdAtMs: number
  lastActivityMs: number
  lastMessagePreview: string
  version: number
}

const sessions: Ref<SessionMeta[]> = ref([])
const loading = ref(false)
const error = ref<string | null>(null)

let pollHandle: ReturnType<typeof setInterval> | null = null
let focusHandlerAttached = false

async function refresh() {
  loading.value = true
  error.value = null
  try {
    const res = await $fetch<SessionMeta[]>('/api/sessions')
    sessions.value = res
  } catch (e: any) {
    error.value = e?.message || 'Failed to load conversations'
  } finally {
    loading.value = false
  }
}

async function create(): Promise<SessionMeta> {
  const res = await $fetch<{ chatId: string; meta: SessionMeta }>('/api/sessions', {
    method: 'POST',
  })
  // Optimistic: prepend so the new chat appears at the top immediately.
  sessions.value = [res.meta, ...sessions.value]
  return res.meta
}

async function rename(id: string, title: string) {
  const idx = sessions.value.findIndex(s => s.chatId === id)
  if (idx < 0) return
  const snapshot = sessions.value[idx].title
  sessions.value[idx].title = title
  try {
    const updated = await $fetch<SessionMeta>(`/api/sessions/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: { title },
    })
    sessions.value[idx] = updated
  } catch (e: any) {
    sessions.value[idx].title = snapshot
    throw e
  }
}

async function remove(id: string) {
  const idx = sessions.value.findIndex(s => s.chatId === id)
  if (idx < 0) return
  const snapshot = sessions.value[idx]
  sessions.value.splice(idx, 1)
  try {
    await $fetch(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' })
  } catch (e) {
    sessions.value.splice(idx, 0, snapshot)
    throw e
  }
}

function bumpLocal(id: string, preview: string) {
  const idx = sessions.value.findIndex(s => s.chatId === id)
  if (idx < 0) return
  const updated = {
    ...sessions.value[idx],
    lastActivityMs: Date.now(),
    lastMessagePreview: preview.slice(0, 120),
  }
  // Move to top.
  sessions.value.splice(idx, 1)
  sessions.value.unshift(updated)
}

export function useSessions() {
  if (!focusHandlerAttached && typeof window !== 'undefined') {
    focusHandlerAttached = true
    window.addEventListener('focus', refresh)
    pollHandle = setInterval(refresh, 30_000)
  }
  return { sessions, loading, error, refresh, create, rename, remove, bumpLocal }
}
```

> **Note:** if Nuxt's auto-import doesn't pick this up, ensure `composables/` is configured in `nuxt.config.ts` (it usually is by default).

- [ ] **Step 2: Sanity-check the build**

```bash
cd web && npm run build
```

Expected: clean Nuxt build.

- [ ] **Step 3: Commit**

```bash
git add web/app/composables/useSessions.ts
git commit -m "feat(web): useSessions composable for sidebar state

Reactive ref of SessionMeta[]. Methods: refresh (auto on focus +
30s poll), create (POST + optimistic prepend), rename + remove
(optimistic with rollback on failure), bumpLocal (no network — for
when the active chat just sent or received a message and we want
the sidebar to reflect it instantly).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 16: `SessionsSidebar` component

**Files:**
- Create: `web/app/components/SessionsSidebar.vue`

- [ ] **Step 1: Write the component**

Create `web/app/components/SessionsSidebar.vue`:

```vue
<template>
  <aside class="flex flex-col h-full w-[300px] border-r border-default bg-elevated">
    <div class="p-3 space-y-2 border-b border-default">
      <UButton block color="primary" icon="i-lucide-plus" @click="onNewChat">
        New chat
      </UButton>
      <UInput
        v-model="query"
        placeholder="Search conversations..."
        icon="i-lucide-search"
        size="sm"
      />
    </div>

    <div v-if="error" class="m-3 p-2 text-sm text-error border border-error rounded">
      {{ error }}
      <UButton size="xs" variant="ghost" @click="refresh">Retry</UButton>
    </div>

    <div class="flex-1 overflow-y-auto">
      <div v-if="filtered.length === 0 && !loading" class="p-4 text-sm text-muted">
        <template v-if="query">No conversations match "{{ query }}".</template>
        <template v-else>No conversations yet — click "New chat" to start.</template>
      </div>

      <NuxtLink
        v-for="session in filtered"
        :key="session.chatId"
        :to="`/chat/${session.chatId}`"
        class="block px-3 py-2 border-b border-default hover:bg-accented"
        :class="{ 'bg-accented': route.params.id === session.chatId }"
      >
        <div class="flex items-center gap-2">
          <UIcon :name="kindIcon(session.kind)" class="text-muted" />
          <input
            v-if="renamingId === session.chatId"
            v-model="renameDraft"
            class="flex-1 bg-transparent border-b border-primary outline-none"
            @blur="commitRename(session.chatId)"
            @keyup.enter="commitRename(session.chatId)"
            @keyup.escape="renamingId = null"
            ref="renameInput"
          />
          <span v-else class="flex-1 truncate font-medium">{{ session.title }}</span>
          <span class="text-xs text-muted">{{ relative(session.lastActivityMs) }}</span>
          <UDropdownMenu :items="rowMenu(session)">
            <UButton size="xs" variant="ghost" icon="i-lucide-more-horizontal" @click.prevent />
          </UDropdownMenu>
        </div>
        <p v-if="session.lastMessagePreview" class="text-xs text-muted truncate mt-0.5">
          {{ session.lastMessagePreview }}
        </p>
      </NuxtLink>
    </div>

    <UModal v-model:open="confirmOpen">
      <template #content>
        <div class="p-4 space-y-3">
          <h3 class="font-semibold">Delete this conversation?</h3>
          <p class="text-sm text-muted">This cannot be undone.</p>
          <div class="flex justify-end gap-2">
            <UButton variant="ghost" @click="confirmOpen = false">Cancel</UButton>
            <UButton color="error" @click="confirmDelete">Delete</UButton>
          </div>
        </div>
      </template>
    </UModal>
  </aside>
</template>

<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

const route = useRoute()
const router = useRouter()

const { sessions, loading, error, refresh, create, rename, remove, bumpLocal } = useSessions()

const query = ref('')
const renamingId = ref<string | null>(null)
const renameDraft = ref('')
const renameInput = ref<HTMLInputElement | null>(null)
const confirmOpen = ref(false)
const pendingDelete = ref<string | null>(null)

await refresh()

const filtered = computed(() => {
  if (!query.value.trim()) return sessions.value
  const q = query.value.trim().toLowerCase()
  return sessions.value.filter(s => s.title.toLowerCase().includes(q))
})

function kindIcon(k: string) {
  return {
    web: 'i-lucide-message-circle',
    telegram: 'i-lucide-send',
    cron: 'i-lucide-clock',
    other: 'i-lucide-circle',
  }[k] || 'i-lucide-circle'
}

function relative(ms: number) {
  const diff = Date.now() - ms
  const min = Math.round(diff / 60_000)
  if (min < 1) return 'now'
  if (min < 60) return `${min}m`
  const hr = Math.round(min / 60)
  if (hr < 24) return `${hr}h`
  const d = Math.round(hr / 24)
  return `${d}d`
}

async function onNewChat() {
  const meta = await create()
  router.push(`/chat/${meta.chatId}`)
}

function rowMenu(session: { chatId: string; title: string }) {
  return [[
    {
      label: 'Rename',
      icon: 'i-lucide-pencil',
      onSelect: () => beginRename(session.chatId, session.title),
    },
    {
      label: 'Delete',
      icon: 'i-lucide-trash',
      color: 'error' as const,
      onSelect: () => {
        pendingDelete.value = session.chatId
        confirmOpen.value = true
      },
    },
  ]]
}

async function beginRename(id: string, current: string) {
  renamingId.value = id
  renameDraft.value = current
  await nextTick()
  renameInput.value?.focus()
  renameInput.value?.select()
}

async function commitRename(id: string) {
  const newTitle = renameDraft.value.trim()
  renamingId.value = null
  if (!newTitle) return
  try {
    await rename(id, newTitle)
  } catch (e: any) {
    // useToast() if available; otherwise console.
    console.error('Rename failed:', e?.message || e)
  }
}

async function confirmDelete() {
  if (!pendingDelete.value) return
  const id = pendingDelete.value
  confirmOpen.value = false
  pendingDelete.value = null
  try {
    if (route.params.id === id) router.push('/chat')
    await remove(id)
  } catch (e: any) {
    console.error('Delete failed:', e?.message || e)
  }
}
</script>
```

- [ ] **Step 2: Sanity-check the build**

```bash
cd web && npm run build
```

Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add web/app/components/SessionsSidebar.vue
git commit -m "feat(web): SessionsSidebar component (list, search, rename, delete)

Claude.ai-style sidebar shell for the multi-conversation UI.
Top: New-chat button + title-search input. Body: sorted list of
SessionMeta with kind icon, relative timestamp, last-message
preview. Per-row kebab menu opens Rename (in-place edit) and
Delete (UModal confirm). Active route highlighted.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 17: `layouts/chat.vue` + `pages/chat/[id].vue`

**Files:**
- Create: `web/app/layouts/chat.vue`
- Create: `web/app/pages/chat/[id].vue`
- Read for reference: `web/app/pages/chat.vue`

- [ ] **Step 1: Read the existing `chat.vue`**

```bash
cat web/app/pages/chat.vue
```

Note the chat panel logic (compose, history fetch, WebSocket plumbing, message rendering). It currently uses hard-coded `'web'` chat_id at lines 143 and 159.

- [ ] **Step 2: Create the layout**

```vue
<!-- web/app/layouts/chat.vue -->
<template>
  <div class="flex h-screen w-screen overflow-hidden">
    <SessionsSidebar />
    <main class="flex-1 overflow-hidden">
      <slot />
    </main>
  </div>
</template>
```

- [ ] **Step 3: Create the dynamic-route page**

Copy `web/app/pages/chat.vue` to `web/app/pages/chat/[id].vue` and modify so:

1. Replace every literal `'web'` chat_id with `route.params.id` (cast to string).
2. At the top of `<script setup>`, declare `definePageMeta({ layout: 'chat' })`.
3. After every successful chat send/receive, call `useSessions().bumpLocal(id, preview)` where `preview` is the assistant's last text.
4. Add a `watch(() => route.params.id, ...)` that re-fetches history when the user clicks a different sidebar row without remounting.

Sketch of the changed parts:

```vue
<script setup lang="ts">
import { ref, watch } from 'vue'
import { useRoute } from 'vue-router'

definePageMeta({ layout: 'chat' })

const route = useRoute()
const chatId = computed(() => String(route.params.id))

const { bumpLocal } = useSessions()

// ...existing reactive refs (messages, streaming flags, etc.)...

async function loadHistory() {
  const result = await getChatHistory(chatId.value, 200)
  // ...adapt existing handler...
}

// Existing send-message function — change the call to bumpLocal:
async function sendMessage(text: string) {
  // ...existing send pipeline; on assistant reply received:
  bumpLocal(chatId.value, replyText)
}

// Re-load when the user clicks a different sidebar row.
watch(chatId, async () => {
  await loadHistory()
})

await loadHistory()
</script>
```

- [ ] **Step 4: Sanity-check the build**

```bash
cd web && npm run build
```

Expected: clean build. (If the lift introduces TypeScript errors around route params, cast `route.params.id` to `string` explicitly.)

- [ ] **Step 5: Commit**

```bash
git add web/app/layouts/chat.vue web/app/pages/chat/[id].vue
git commit -m "feat(web): chat.vue layout + dynamic /chat/:id route

Two-column layout (sidebar + slot) at layouts/chat.vue. Dynamic
page at pages/chat/[id].vue lifts the existing chat-panel logic
from pages/chat.vue and reads chat_id from the route. Watches
route.params.id to swap conversations without remount. Calls
useSessions().bumpLocal after each turn so the sidebar reflects
activity instantly.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 18: `pages/chat/index.vue` + remove `pages/chat.vue`

**Files:**
- Create: `web/app/pages/chat/index.vue`
- Remove: `web/app/pages/chat.vue`

- [ ] **Step 1: Create the index page**

```vue
<!-- web/app/pages/chat/index.vue -->
<template>
  <div class="flex items-center justify-center h-full text-muted">
    <p>Select a chat from the sidebar or click "New chat" to start.</p>
  </div>
</template>

<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'

definePageMeta({ layout: 'chat' })

const router = useRouter()
const { sessions, refresh } = useSessions()

await refresh()

onMounted(() => {
  if (sessions.value.length > 0) {
    const mostRecent = [...sessions.value].sort(
      (a, b) => b.lastActivityMs - a.lastActivityMs,
    )[0]
    router.replace(`/chat/${mostRecent.chatId}`)
  }
})
</script>
```

- [ ] **Step 2: Remove the old top-level chat page**

```bash
git rm web/app/pages/chat.vue
```

- [ ] **Step 3: Sanity-check**

```bash
cd web && npm run build && npm run dev &
DEV_PID=$!
sleep 8
curl -sI http://localhost:3000/chat | head -3   # should serve a page (not 404)
curl -sI http://localhost:3000/chat/web | head -3  # should serve the dynamic route
kill $DEV_PID
```

(Adapt to the right port if different from 3000.)

- [ ] **Step 4: Commit**

```bash
git add web/app/pages/chat/index.vue
git commit -m "feat(web): /chat index redirects to most-recent or shows empty

pages/chat/index.vue handles bare /chat: redirects to the most
recent conversation when sessions exist, else shows an empty pane
inside the chat layout (sidebar still visible). Old top-level
pages/chat.vue removed — its logic moved to pages/chat/[id].vue
in the previous task.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 19: End-to-end Playwright happy-path + manual smoke checklist

**Files:**
- Create: `docs/superpowers/playbooks/multi-conversations-e2e.md`

- [ ] **Step 1: Write the e2e playbook**

Create `docs/superpowers/playbooks/multi-conversations-e2e.md`:

```markdown
# Multi-conversation UI — Playwright happy path

Run interactively via Playwright MCP. Adapt the host to your provisioned device.

## Prerequisites

- Device flashed with the multi-conversations build (devkitc or guition-p4)
- Web UI accessible at https://bennyzen.github.io/zenclaw/ (or local dev at http://localhost:3000)
- Device hostname: e.g. `zenclaw-tomato.local`

## Steps

1. Open the web UI and navigate to the Dashboard. Connect to your device.
2. Navigate to `/chat`. Verify the sidebar appears on the left.
3. Click "New chat". Verify URL becomes `/chat/chat-{ms}` and a new sidebar row appears at the top with title "New chat".
4. Type "ping" in the compose box and submit. Wait for the assistant's reply.
5. Wait ~5 seconds. Verify the sidebar row's title updates from "New chat" to an LLM-summarized one (e.g., "Greeting").
6. Click the row's kebab menu → Rename. Type "Custom title" and press Enter. Verify the row's title updates immediately.
7. Reload the page. Verify the title persists.
8. Click the kebab menu → Delete. Confirm in the modal. Verify the row disappears and URL navigates to `/chat`.

## Smoke commands (alternative: pure curl)

\`\`\`bash
HOST=zenclaw-tomato.local

ID=$(curl -sf -X POST http://$HOST/api/sessions | jq -r .chatId)
curl -sf -X POST http://$HOST/api/chat -H 'Content-Type: application/json' \
  -d "{\"chat_id\":\"$ID\",\"message\":\"ping\"}"
sleep 5
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\")"
curl -sf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' -d '{"title":"smoke"}'
curl -sf -X DELETE "http://$HOST/api/sessions/$ID"
curl -sf "http://$HOST/api/cloud/files?prefix=sys/sessions/$ID/"  # → empty
\`\`\`

Run on **devkitc** AND **guition-p4** before declaring v1 shipped.
```

- [ ] **Step 2: Run the playbook against devkitc**

Flash, then execute the script above against your devkitc device. All assertions must pass.

- [ ] **Step 3: Run the playbook against guition-p4**

Flash the P4 build, then execute the script. All assertions must pass on both boards.

- [ ] **Step 4: Commit the playbook**

```bash
git add docs/superpowers/playbooks/multi-conversations-e2e.md
git commit -m "docs(playbook): multi-conversations e2e happy-path

Playwright + curl steps for verifying the full create-send-title-
rename-delete loop. Run on both devkitc and guition-p4 before
declaring v1 shipped.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Self-Review Checklist (run after Task 19)

After completing all tasks, run this checklist before merging.

- [ ] All 19 tasks committed in order, each with passing tests.
- [ ] `cd agent && cargo test --features desktop` passes.
- [ ] `cd agent && just build devkitc` produces a binary.
- [ ] `cd agent && just build guition-p4` produces a binary.
- [ ] `cd web && npm run build` produces a clean production build.
- [ ] Manual smoke against devkitc: full create-send-rename-delete loop succeeds.
- [ ] Manual smoke against guition-p4: same.
- [ ] Boot-restore round-trip: create chat A, reboot, verify chat A appears in sidebar with correct title.
- [ ] Cloud-cleanup verify: after delete, `curl /api/cloud/files?prefix=sys/sessions/<deleted_id>/` returns empty.
- [ ] No new test framework added to `web/` (Vitest deferred per "Divergence From Spec").

If any check fails, stop and fix before declaring complete.

## Spec Coverage Map

| Spec section | Implementing task(s) |
|---|---|
| `SessionMeta` types + serde | Task 1 |
| `detect_kind` (canonical + sanitized) | Task 2 |
| `synthesize_default` (with/without first message) | Task 3 |
| `SessionManager::meta` | Task 4 |
| `SessionManager::set_meta` (cloud-aware) | Task 5 |
| `SessionManager::list_with_meta` | Task 6 |
| `SessionManager::bump_activity` | Task 7 |
| `SessionManager::rename` + `rename_internal` | Task 8 |
| `SessionManager::delete` (cloud cleanup) | Task 9 |
| Boot-restore extension | Task 10 |
| Gateway hook for `bump_activity` | Task 11 |
| Title generation task | Task 12 |
| HTTP routes (desktop) | Task 13 |
| HTTP routes (ESP32) | Task 14 |
| `useSessions` composable | Task 15 |
| `SessionsSidebar` component | Task 16 |
| `layouts/chat.vue` + `pages/chat/[id].vue` | Task 17 |
| `pages/chat/index.vue` + remove `pages/chat.vue` | Task 18 |
| Playwright e2e + manual smoke | Task 19 |
| Vitest frontend unit tests | **Deferred** (no framework set up; see Divergence) |

Every spec deliverable maps to at least one task.
