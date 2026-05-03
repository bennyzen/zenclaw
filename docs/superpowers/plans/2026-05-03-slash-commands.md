# Slash Commands v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a code-owned slash-command layer (`/new`, `/clear`, `/status`, `/help`) that works identically on Telegram and the web chat, and have ZenClaw self-register the command list with Telegram on every boot via `setMyCommands` so the BotFather menu can never drift from what the agent actually supports.

**Architecture:** Single new module `agent/src/core/commands.rs` owns the parser, the four executors, and the menu list — one `const` slice drives both dispatch and Telegram menu registration, making drift impossible. Interception happens at the top of `Gateway::chat_with_events` before auto-compaction so `/clear` doesn't summarize history right before wiping it. A small `HostFacts` trait (impls in `main.rs` for ESP32, `desktop/host_facts.rs` for desktop) bridges Gateway to platform-specific reads (heap, IP, link, RSSI, uptime). `SessionManager::clear()` is extended to be cloud-aware (drop cache keys + best-effort S3 delete) so `/clear` doesn't silently no-op once cloud persistence is enabled.

**Tech Stack:** Rust 2021, `esp-idf-svc` (ESP32), `tokio` (desktop only), `serde_json`. Tests run with `cargo test --features desktop --no-default-features --lib` from `agent/`. ESP32 build verified with `just build devkitc` from `agent/`.

**Spec:** [`docs/superpowers/specs/2026-05-03-slash-commands-design.md`](../specs/2026-05-03-slash-commands-design.md) (commit `d958574`).

---

### Task 1: Bootstrap the `commands` module

**Files:**
- Create: `agent/src/core/commands.rs`
- Modify: `agent/src/core/mod.rs`

- [ ] **Step 1: Create `agent/src/core/commands.rs` with module-level docs only**

```rust
//! Slash-command parser, executors, and Telegram menu list.
//!
//! Single source of truth for the four user-issued commands the agent
//! recognizes today: `/new`, `/clear`, `/status`, `/help`. The same
//! `menu()` const slice is consumed by both `parse()` (dispatch) and
//! `Poller::set_my_commands` (Telegram menu registration on boot) —
//! drift between the two is impossible by construction.
//!
//! Hook point: `Gateway::chat_with_events` calls `parse()` before
//! auto-compaction. Recognized commands skip the LLM entirely.
```

- [ ] **Step 2: Register the module in `agent/src/core/mod.rs`**

Find the existing `pub mod` declarations (search for `pub mod gateway;`) and add `pub mod commands;` next to them, alphabetically. The diff should be a single line.

- [ ] **Step 3: Verify the project compiles**

Run: `cd agent && cargo check --features desktop --no-default-features --quiet`
Expected: exit 0, no output.

- [ ] **Step 4: Commit**

```bash
git add agent/src/core/commands.rs agent/src/core/mod.rs
git commit -m "feat(commands): scaffold slash-command module"
```

---

### Task 2: Define `Command` enum and `menu()` const slice

**Files:**
- Modify: `agent/src/core/commands.rs`

- [ ] **Step 1: Write the failing test**

Append to `commands.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_lists_four_commands_with_descriptions() {
        let m = menu();
        assert_eq!(m.len(), 4);
        let names: Vec<&str> = m.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["new", "clear", "status", "help"]);
        for (_, desc) in m {
            assert!(!desc.is_empty(), "every command needs a description");
        }
    }
}
```

- [ ] **Step 2: Run test to confirm it fails**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands::tests::menu_lists_four_commands_with_descriptions 2>&1 | tail -5`
Expected: compile error, `menu` not found.

- [ ] **Step 3: Add the enum and const**

Insert above the `#[cfg(test)] mod tests` block:

```rust
/// User-issued slash commands recognized by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Alias for `Clear`.
    New,
    /// Wipe the current session for this `chat_id`. Preserves `SessionState`
    /// (model_override etc.) — only the conversation history goes.
    Clear,
    /// Render a markdown table of live device facts.
    Status,
    /// Static list of available commands.
    Help,
}

/// Single source of truth for the BotFather menu and the parser.
///
/// Order matters — this list is what users see in Telegram's `/` menu,
/// so the most-used command (`/new`) goes first.
pub fn menu() -> &'static [(&'static str, &'static str)] {
    &[
        ("new",    "Start a fresh chat (alias for /clear)"),
        ("clear",  "Wipe the current chat history"),
        ("status", "Show device status (heap, link, model)"),
        ("help",   "List available commands"),
    ]
}
```

- [ ] **Step 4: Run the test, confirm it passes**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands::tests::menu_lists_four_commands_with_descriptions 2>&1 | tail -5`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/commands.rs
git commit -m "feat(commands): define Command enum and menu() const"
```

---

### Task 3: Implement `parse()` with all edge cases

**Files:**
- Modify: `agent/src/core/commands.rs`

This task does the full TDD cycle for the parser in one task because each test is tiny and they reinforce each other.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` block:

```rust
#[test]
fn parse_recognizes_all_four_commands() {
    assert_eq!(parse("/new"), Some(Command::New));
    assert_eq!(parse("/clear"), Some(Command::Clear));
    assert_eq!(parse("/status"), Some(Command::Status));
    assert_eq!(parse("/help"), Some(Command::Help));
}

#[test]
fn parse_strips_telegram_botname_suffix() {
    assert_eq!(parse("/status@zenclaw_bot"), Some(Command::Status));
    assert_eq!(parse("/clear@anything"), Some(Command::Clear));
}

#[test]
fn parse_returns_none_for_unknown_commands() {
    assert_eq!(parse("/foo"), None);
    assert_eq!(parse("/"), None);
    assert_eq!(parse("hello"), None);
    assert_eq!(parse(""), None);
}

#[test]
fn parse_ignores_trailing_args() {
    assert_eq!(parse("/clear extra trailing words"), Some(Command::Clear));
    assert_eq!(parse("/status\nmore stuff"), Some(Command::Status));
}

#[test]
fn parse_only_matches_at_start() {
    assert_eq!(parse("not a command /clear"), None);
    assert_eq!(parse(" /clear"), None); // leading space — not at start
}

/// Drift guard: every command in `menu()` must round-trip through `parse()`.
#[test]
fn menu_entries_all_parse() {
    for (name, _) in menu() {
        let with_slash = format!("/{}", name);
        assert!(
            parse(&with_slash).is_some(),
            "menu lists `/{}` but parse() does not recognize it",
            name,
        );
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands:: 2>&1 | tail -10`
Expected: compile error, `parse` not found.

- [ ] **Step 3: Implement `parse()`**

Insert above the `#[cfg(test)] mod tests` block (right after `pub fn menu()`):

```rust
/// Parse a user message into a recognized command.
///
/// Matches only when the message *starts* with `/<name>`. Trailing
/// arguments are ignored (no v1 command takes args). The Telegram
/// group-chat suffix `@<botname>` after the command name is stripped
/// before lookup.
pub fn parse(text: &str) -> Option<Command> {
    let rest = text.strip_prefix('/')?;

    // First whitespace OR newline ends the command token.
    let token_end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let token = &rest[..token_end];
    if token.is_empty() {
        return None;
    }

    // Strip Telegram group-chat suffix: "/status@zenclaw_bot".
    let name = token.split('@').next().unwrap_or(token);

    match name {
        "new"    => Some(Command::New),
        "clear"  => Some(Command::Clear),
        "status" => Some(Command::Status),
        "help"   => Some(Command::Help),
        _        => None,
    }
}
```

- [ ] **Step 4: Run all command tests, confirm they pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands:: 2>&1 | tail -10`
Expected: `test result: ok. 7 passed`.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/commands.rs
git commit -m "feat(commands): implement parse() with botname-suffix and trailing-args handling"
```

---

### Task 4: Define `LinkKind`, `RuntimeFacts`, and `HostFacts` trait

**Files:**
- Modify: `agent/src/core/commands.rs`

- [ ] **Step 1: Add the types**

Insert above the existing `pub fn menu()`:

```rust
use std::sync::Arc;

/// Network link description for `/status`.
#[derive(Debug, Clone)]
pub enum LinkKind {
    Wifi { ssid: String, rssi: Option<i32> },
    Ethernet,
    Desktop,
}

/// Live device facts assembled by `Gateway::runtime_facts(chat_id)` and
/// passed to `execute()`. Stable fields (`agent_name`, `platform`,
/// session size, model) come from `Gateway` directly; live fields come
/// from the `HostFacts` trait so platform-specific reads stay out of
/// `commands.rs` itself.
#[derive(Debug, Clone)]
pub struct RuntimeFacts {
    pub hostname: String,
    pub ip: Option<String>,
    pub link: LinkKind,
    pub free_internal_heap: Option<u32>,
    pub free_psram: Option<u32>,
    pub uptime_secs: u64,
    pub agent_name: String,
    pub platform: &'static str,
    pub session_bytes: u64,
    pub session_entries: usize,
    pub model: String,
}

/// Bridge from `Gateway` to platform-specific runtime reads.
///
/// `Esp32HostFacts` (in `main.rs`) reads heap/RSSI from `esp_idf_svc`.
/// `DesktopHostFacts` (in `desktop/host_facts.rs`) returns desktop-shaped
/// values (heap = `None`, link = `Desktop`).
pub trait HostFacts: Send + Sync {
    fn hostname(&self) -> String;
    fn ip(&self) -> Option<String>;
    fn link(&self) -> LinkKind;
    fn free_internal_heap(&self) -> Option<u32>;
    fn free_psram(&self) -> Option<u32>;
    fn uptime_secs(&self) -> u64;
}

/// Detect the build platform. Replaces the broken `cfg!(target_os)`
/// ladder in `session_tools.rs::do_status` which reported `unknown` on
/// every ESP32 build.
pub fn detect_platform() -> &'static str {
    if cfg!(target_os = "espidf") {
        if cfg!(target_arch = "xtensa") {
            "esp32-s3"
        } else if cfg!(target_arch = "riscv32") {
            "esp32-p4"
        } else {
            "espidf"
        }
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}
```

- [ ] **Step 2: Add `FakeHostFacts` to the test module**

Append inside `mod tests`:

```rust
pub(super) struct FakeHostFacts {
    pub hostname: String,
    pub ip: Option<String>,
    pub link: LinkKind,
    pub heap: Option<u32>,
    pub psram: Option<u32>,
    pub uptime: u64,
}

impl FakeHostFacts {
    pub fn new() -> Self {
        Self {
            hostname: "test-host".to_string(),
            ip: Some("10.0.0.1".to_string()),
            link: LinkKind::Wifi { ssid: "test".to_string(), rssi: Some(-55) },
            heap: Some(120_000),
            psram: Some(7_500_000),
            uptime: 42,
        }
    }
}

impl HostFacts for FakeHostFacts {
    fn hostname(&self) -> String { self.hostname.clone() }
    fn ip(&self) -> Option<String> { self.ip.clone() }
    fn link(&self) -> LinkKind { self.link.clone() }
    fn free_internal_heap(&self) -> Option<u32> { self.heap }
    fn free_psram(&self) -> Option<u32> { self.psram }
    fn uptime_secs(&self) -> u64 { self.uptime }
}

#[test]
fn detect_platform_returns_known_string_on_test_host() {
    let p = detect_platform();
    // Test host is always linux/macos/windows — never the espidf fallback.
    assert!(
        matches!(p, "linux" | "macos" | "windows"),
        "expected host platform, got {:?}",
        p,
    );
}
```

- [ ] **Step 3: Run tests, confirm pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands:: 2>&1 | tail -10`
Expected: `test result: ok. 8 passed`.

- [ ] **Step 4: Commit**

```bash
git add agent/src/core/commands.rs
git commit -m "feat(commands): add LinkKind, RuntimeFacts, HostFacts trait, detect_platform"
```

---

### Task 5: Implement `execute()` for `/help`

**Files:**
- Modify: `agent/src/core/commands.rs`

`/help` is the simplest executor — no I/O, no state. We start here so the `execute()` function shape is settled before the harder commands.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
#[tokio::test]
async fn execute_help_lists_all_commands_with_descriptions() {
    let facts = make_fake_runtime_facts();
    let out = execute(Command::Help, &facts).await;
    for (name, desc) in menu() {
        assert!(out.contains(&format!("/{}", name)),
            "expected /{} in /help output, got: {}", name, out);
        assert!(out.contains(desc),
            "expected description {:?} in /help output", desc);
    }
}

fn make_fake_runtime_facts() -> RuntimeFacts {
    let h = FakeHostFacts::new();
    RuntimeFacts {
        hostname: h.hostname(),
        ip: h.ip(),
        link: h.link(),
        free_internal_heap: h.free_internal_heap(),
        free_psram: h.free_psram(),
        uptime_secs: h.uptime_secs(),
        agent_name: "TestAgent".to_string(),
        platform: "test",
        session_bytes: 0,
        session_entries: 0,
        model: "test-model".to_string(),
    }
}
```

- [ ] **Step 2: Run test, confirm fail**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands::tests::execute_help 2>&1 | tail -5`
Expected: compile error, `execute` not found.

- [ ] **Step 3: Implement `execute()` with only the `Help` arm**

Insert above the `#[cfg(test)] mod tests` block. Note: this is the public function the gateway calls. Other arms come in following tasks.

```rust
/// Execute a parsed slash command. Returns the user-visible reply.
///
/// `async` for forward-compat — v1 ops are sync but `/restart`, `/model`
/// (deferred to v2) will need NVS / HTTP I/O. Async now avoids breaking
/// callers later.
///
/// Note: `Clear` and `New` need access to `SessionManager`, which is on
/// `Gateway`. The signature in subsequent tasks will grow to include
/// `&SessionManager` + cloud handles. We start with the simplest shape
/// and extend.
pub async fn execute(cmd: Command, facts: &RuntimeFacts) -> String {
    match cmd {
        Command::Help => render_help(),
        // Other arms wired in later tasks.
        _ => format!("(command {:?} not yet implemented)", cmd),
    }
}

fn render_help() -> String {
    let mut s = String::from("**Available commands:**\n\n");
    for (name, desc) in menu() {
        s.push_str(&format!("- `/{}` — {}\n", name, desc));
    }
    s
}
```

- [ ] **Step 4: Run tests, confirm pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands:: 2>&1 | tail -5`
Expected: `test result: ok. 9 passed`.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/commands.rs
git commit -m "feat(commands): implement /help executor"
```

---

### Task 6: Implement `execute()` for `/status`

**Files:**
- Modify: `agent/src/core/commands.rs`

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
#[tokio::test]
async fn execute_status_renders_full_facts() {
    let facts = make_fake_runtime_facts();
    let out = execute(Command::Status, &facts).await;
    // Header
    assert!(out.contains("TestAgent"), "agent_name missing: {}", out);
    // Identity rows
    assert!(out.contains("test-host"));
    assert!(out.contains("10.0.0.1"));
    // Platform fix — the bug we set out to fix.
    assert!(out.contains("test"));
    // Link
    assert!(out.contains("test") && out.contains("-55"),
        "WiFi SSID and RSSI missing: {}", out);
    // Heap (formatted in KB or MB)
    assert!(out.contains("120"), "heap missing: {}", out);
    assert!(out.contains("7"), "psram missing: {}", out);
    // Model
    assert!(out.contains("test-model"));
}

#[tokio::test]
async fn execute_status_renders_em_dash_for_missing_fields() {
    let mut facts = make_fake_runtime_facts();
    facts.ip = None;
    facts.free_internal_heap = None;
    facts.free_psram = None;
    facts.link = LinkKind::Ethernet;
    let out = execute(Command::Status, &facts).await;
    // Em-dash placeholder (—) appears for unknown fields rather than
    // dropping the row entirely.
    assert!(out.contains("—"), "expected em-dash for missing fields: {}", out);
    assert!(out.contains("Ethernet"), "Ethernet link missing: {}", out);
}
```

- [ ] **Step 2: Run, confirm fail**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands::tests::execute_status 2>&1 | tail -5`
Expected: tests fail (output contains `not yet implemented`).

- [ ] **Step 3: Add the `Status` arm and `render_status` helper**

In `execute()`, replace the `Command::Help => render_help()` arm so the match becomes:

```rust
    match cmd {
        Command::Help   => render_help(),
        Command::Status => render_status(facts),
        _ => format!("(command {:?} not yet implemented)", cmd),
    }
```

Then add the helper functions (above `mod tests`):

```rust
fn render_status(f: &RuntimeFacts) -> String {
    fn or_dash<T: std::fmt::Display>(v: Option<T>) -> String {
        v.map(|x| x.to_string()).unwrap_or_else(|| "—".to_string())
    }
    fn fmt_kb(bytes: Option<u32>) -> String {
        match bytes {
            Some(b) if b >= 1_000_000 => format!("{} MB", b / 1_000_000),
            Some(b) => format!("{} KB", b / 1024),
            None => "—".to_string(),
        }
    }
    let link = match &f.link {
        LinkKind::Wifi { ssid, rssi } => format!("WiFi {} ({} dBm)", ssid, or_dash(*rssi)),
        LinkKind::Ethernet => "Ethernet".to_string(),
        LinkKind::Desktop  => "Desktop".to_string(),
    };
    format!(
        "**{} Status**\n\n\
         | Field | Value |\n\
         |---|---|\n\
         | Hostname | `{}` |\n\
         | IP | {} |\n\
         | Link | {} |\n\
         | Platform | `{}` |\n\
         | Free internal heap | {} |\n\
         | Free PSRAM | {} |\n\
         | Uptime | {}s |\n\
         | Model | `{}` |\n\
         | Session | {} bytes, {} entries |\n",
        f.agent_name,
        f.hostname,
        or_dash(f.ip.as_deref()),
        link,
        f.platform,
        fmt_kb(f.free_internal_heap),
        fmt_kb(f.free_psram),
        f.uptime_secs,
        f.model,
        f.session_bytes,
        f.session_entries,
    )
}
```

- [ ] **Step 4: Run, confirm pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands:: 2>&1 | tail -5`
Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/commands.rs
git commit -m "feat(commands): implement /status executor with em-dash for missing fields"
```

---

### Task 7: Extend `execute()` signature for `/clear` and `/new`, and make `SessionManager::clear()` cloud-aware

**Files:**
- Modify: `agent/src/core/sessions/mod.rs`
- Modify: `agent/src/core/commands.rs`

`/clear` and `/new` (alias) need to call `SessionManager::clear(chat_id)`. The existing `clear()` only deletes the local file, which silently no-ops in cloud mode. We extend it to drop matching cache keys and (when an `ObjectStore` is supplied) issue best-effort S3 deletes.

- [ ] **Step 1: Write a test for cloud-aware clear**

In `agent/src/core/sessions/mod.rs`, find the existing `#[cfg(test)] mod tests` block (search for `assert!(state.model_override.is_none())` to find it). Add this test inside that block:

```rust
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
```

If `tempfile` isn't already a dev-dep, check `Cargo.toml` `[dev-dependencies]` — most likely it is, since other tests use temp dirs. If absent, add `tempfile = "3"` to `[dev-dependencies]`.

If `Replicator::new_for_test()` doesn't exist, search for an existing test-construction pattern in `agent/src/core/cloud/replicator.rs::tests` (the file has its own tests at line 209+) and use whichever construction pattern works there — if `new_for_test` doesn't exist, add a `#[cfg(test)] pub(crate) fn new_for_test() -> Self` that builds the simplest viable Replicator. Do this in a separate prep step before running the tests.

- [ ] **Step 2: Run, confirm fail**

Run: `cd agent && cargo test --features desktop --no-default-features --lib sessions::tests::clear_ 2>&1 | tail -10`
Expected: failures (the `clear_in_cloud_mode_drops_cache_keys_for_chat_id` test).

- [ ] **Step 3: Extend `SessionManager::clear()`**

Find `pub fn clear(&self, chat_id: &str)` at `agent/src/core/sessions/mod.rs:519` and replace with:

```rust
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
        store: &dyn crate::core::cloud::ObjectStore,
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
```

The `ObjectStore` re-export path may be `crate::core::cloud::client::ObjectStore` — check `agent/src/core/cloud/mod.rs` for what's actually re-exported. Adjust the use path as needed.

- [ ] **Step 4: Run, confirm sessions tests pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib sessions:: 2>&1 | tail -10`
Expected: all sessions tests pass, including the two new ones.

- [ ] **Step 5: Now extend `execute()` to handle `/new` and `/clear`**

The `execute()` signature needs to grow. Update `commands.rs`:

```rust
pub async fn execute(
    cmd: Command,
    chat_id: &str,
    facts: &RuntimeFacts,
    sessions: &crate::core::sessions::SessionManager,
    cloud_store: Option<&dyn crate::core::cloud::ObjectStore>,
) -> String {
    match cmd {
        Command::Help   => render_help(),
        Command::Status => render_status(facts),
        Command::New | Command::Clear => clear_session(chat_id, sessions, cloud_store),
    }
}

fn clear_session(
    chat_id: &str,
    sessions: &crate::core::sessions::SessionManager,
    store: Option<&dyn crate::core::cloud::ObjectStore>,
) -> String {
    let result = match store {
        Some(s) => sessions.clear_with_store(chat_id, s),
        None    => sessions.clear(chat_id),
    };
    match result {
        Ok(()) => "Session cleared.".to_string(),
        Err(e) => format!("Failed to clear session: {}", e),
    }
}
```

- [ ] **Step 6: Update existing test calls**

The `execute_help_lists_all_commands_with_descriptions` and `execute_status_*` tests will fail to compile — they call `execute(cmd, &facts)` with the old 2-arg signature.

Update each test to construct a temporary `SessionManager` and pass it:

```rust
fn make_test_sessions() -> (tempfile::TempDir, crate::core::sessions::SessionManager) {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sessions");
    std::fs::create_dir_all(&sub).unwrap();
    let mgr = crate::core::sessions::SessionManager::new(sub.to_str().unwrap());
    (dir, mgr)
}
```

Replace each `execute(cmd, &facts).await` call with:

```rust
let (_tmp, sessions) = make_test_sessions();
execute(cmd, "chat-test", &facts, &sessions, None).await
```

Then add the clear-specific tests:

```rust
#[tokio::test]
async fn execute_clear_deletes_session_file() {
    let (_tmp, sessions) = make_test_sessions();
    let path = format!("{}/abc.jsonl", sessions.sessions_dir());
    std::fs::write(&path, "{}\n").unwrap();

    let facts = make_fake_runtime_facts();
    let out = execute(Command::Clear, "abc", &facts, &sessions, None).await;

    assert!(out.contains("cleared"), "out was: {}", out);
    assert!(!std::path::Path::new(&path).exists(),
        "session file should have been deleted");
}

#[tokio::test]
async fn execute_clear_preserves_model_override() {
    let (_tmp, sessions_owned) = make_test_sessions();
    // SessionManager::set_state is `&mut self`, so we need ownership.
    let mut sessions = sessions_owned;
    sessions.set_state("abc", crate::core::sessions::SessionState {
        turn_count: 0,
        model_override: Some("gpt-4".to_string()),
        last_channel: None,
    });
    let path = format!("{}/abc.jsonl", sessions.sessions_dir());
    std::fs::write(&path, "{}\n").unwrap();

    let facts = make_fake_runtime_facts();
    let _out = execute(Command::Clear, "abc", &facts, &sessions, None).await;

    let st = sessions.get_state("abc");
    assert_eq!(st.model_override, Some("gpt-4".to_string()),
        "model_override must survive /clear");
}

#[tokio::test]
async fn execute_new_is_alias_for_clear() {
    let (_tmp, sessions) = make_test_sessions();
    let path = format!("{}/abc.jsonl", sessions.sessions_dir());
    std::fs::write(&path, "{}\n").unwrap();

    let facts = make_fake_runtime_facts();
    let out = execute(Command::New, "abc", &facts, &sessions, None).await;

    assert!(out.contains("cleared"));
    assert!(!std::path::Path::new(&path).exists());
}
```

- [ ] **Step 7: Run all command tests, confirm pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib commands:: 2>&1 | tail -10`
Expected: `test result: ok. 14 passed`.

- [ ] **Step 8: Commit**

```bash
git add agent/src/core/commands.rs agent/src/core/sessions/mod.rs
git commit -m "feat(commands): implement /clear and /new; make SessionManager::clear cloud-aware"
```

---

### Task 8: Add `host_facts: Arc<dyn HostFacts>` field to `Gateway` + `runtime_facts(chat_id)` helper

**Files:**
- Modify: `agent/src/core/gateway.rs`

- [ ] **Step 1: Add the field to the `Gateway` struct**

Find `pub struct Gateway {` at `gateway.rs:34`. Add after the `cloud_backoff_cap_secs` field, before `active_chats`:

```rust
    /// Bridge to platform-specific runtime reads (heap, link, RSSI, IP).
    /// Populated at construction by `main.rs` (ESP32) or `desktop/run.rs`.
    pub host_facts: Arc<dyn crate::core::commands::HostFacts>,
```

- [ ] **Step 2: Update both constructors to accept and store it**

Modify `Gateway::new` (at `gateway.rs:56`):

```rust
    pub fn new(
        config: Config,
        data_dir: &str,
        runner: Box<dyn LlmRunner>,
        host_facts: Arc<dyn crate::core::commands::HostFacts>,
    ) -> Self {
        Self::new_inner(config, data_dir, runner, None, host_facts)
    }
```

Modify `Gateway::new_with_cloud` (at `gateway.rs:65`):

```rust
    pub fn new_with_cloud(
        config: Config,
        data_dir: &str,
        runner: Box<dyn LlmRunner>,
        cloud: CloudHandles,
        host_facts: Arc<dyn crate::core::commands::HostFacts>,
    ) -> Self {
        Self::new_inner(config, data_dir, runner, Some(cloud), host_facts)
    }
```

Modify `new_inner` (at `gateway.rs:74`) to take and store the new param. Add `host_facts: Arc<dyn crate::core::commands::HostFacts>` to its signature, and add `host_facts,` to the struct literal.

- [ ] **Step 3: Add the `runtime_facts(chat_id)` helper**

Add after `chat_with_events`:

```rust
    /// Build a complete `RuntimeFacts` for the given `chat_id`, combining
    /// `HostFacts` reads (heap, link, RSSI) with state `Gateway` already owns
    /// (session size, model override resolution, agent name, platform).
    pub fn runtime_facts(&self, chat_id: &str) -> crate::core::commands::RuntimeFacts {
        use crate::core::commands::{detect_platform, RuntimeFacts};

        let session_bytes = self.sessions.session_size_bytes(chat_id).unwrap_or(0) as u64;
        let session_entries = self
            .sessions
            .load(chat_id)
            .map(|v| v.len())
            .unwrap_or(0);

        let state = self.sessions.get_state(chat_id);
        let model = state
            .model_override
            .clone()
            .or_else(|| {
                let p = &self.config.providers;
                let default = &p.default;
                p.entries.get(default).map(|cfg| cfg.model.clone())
            })
            .unwrap_or_else(|| "(unset)".to_string());

        RuntimeFacts {
            hostname: self.host_facts.hostname(),
            ip: self.host_facts.ip(),
            link: self.host_facts.link(),
            free_internal_heap: self.host_facts.free_internal_heap(),
            free_psram: self.host_facts.free_psram(),
            uptime_secs: self.host_facts.uptime_secs(),
            agent_name: self.config.agent_name.clone(),
            platform: detect_platform(),
            session_bytes,
            session_entries,
            model,
        }
    }
```

If `config.providers.entries` doesn't exist with that exact name, search for the correct path in `config.rs`. The intent: pull the model name for the default provider.

- [ ] **Step 4: Update existing call sites that construct `Gateway`**

Search for all uses: `grep -rn "Gateway::new\|Gateway::new_with_cloud" agent/src/`. Each call site needs to pass an `Arc<dyn HostFacts>`. There will be at minimum:

- `agent/src/main.rs` (line ~210)
- `agent/src/desktop/run.rs`

At each call site, *temporarily* construct a stub:

```rust
struct StubFacts;
impl crate::core::commands::HostFacts for StubFacts {
    fn hostname(&self) -> String { "unknown".to_string() }
    fn ip(&self) -> Option<String> { None }
    fn link(&self) -> crate::core::commands::LinkKind {
        crate::core::commands::LinkKind::Desktop
    }
    fn free_internal_heap(&self) -> Option<u32> { None }
    fn free_psram(&self) -> Option<u32> { None }
    fn uptime_secs(&self) -> u64 { 0 }
}
let host_facts: std::sync::Arc<dyn crate::core::commands::HostFacts> =
    std::sync::Arc::new(StubFacts);
```

Pass `host_facts` to `Gateway::new` / `Gateway::new_with_cloud`. Real impls go in Tasks 11–12.

Test fixtures (e.g., any `Gateway::new(...)` call in tests) get the same `StubFacts` treatment — keep it inline in each test or extract to a shared test helper.

- [ ] **Step 5: Verify compile + existing tests pass**

```bash
cd agent && cargo test --features desktop --no-default-features --lib 2>&1 | tail -10
```
Expected: 193+14+ tests pass (no regressions in existing 193).

Also verify ESP32 still compiles:
```bash
cd agent && just build devkitc 2>&1 | tail -5
```
Expected: `Compiling zenclaw-agent`... `Finished` (no errors).

- [ ] **Step 6: Commit**

```bash
git add agent/src/core/gateway.rs agent/src/main.rs agent/src/desktop/run.rs
git commit -m "feat(gateway): add host_facts field and runtime_facts(chat_id) helper"
```

---

### Task 9: Hook command interception into `Gateway::chat_with_events`

**Files:**
- Modify: `agent/src/core/gateway.rs`

- [ ] **Step 1: Write an integration test**

If `gateway.rs` already has a `#[cfg(test)] mod tests` block, add inside it; otherwise scaffold one at the bottom of the file.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Reuse the StubFacts from the constructor call sites — or define here.
    struct StubFacts;
    impl crate::core::commands::HostFacts for StubFacts {
        fn hostname(&self) -> String { "test-host".into() }
        fn ip(&self) -> Option<String> { Some("127.0.0.1".into()) }
        fn link(&self) -> crate::core::commands::LinkKind {
            crate::core::commands::LinkKind::Desktop
        }
        fn free_internal_heap(&self) -> Option<u32> { None }
        fn free_psram(&self) -> Option<u32> { None }
        fn uptime_secs(&self) -> u64 { 0 }
    }

    /// Stub runner that panics if called — proves slash commands skip the LLM.
    /// `LlmRunner` trait signature confirmed at `agent/src/core/runner.rs:45-52`:
    /// `async fn call(messages, tools, model_override) -> Result<LlmResponse, RunnerError>`.
    struct PanicRunner;
    #[async_trait::async_trait]
    impl LlmRunner for PanicRunner {
        async fn call(
            &self,
            _messages: &[crate::core::types::Message],
            _tools: &[crate::core::types::ToolDefinition],
            _model_override: Option<&str>,
        ) -> Result<crate::core::runner::LlmResponse, crate::core::runner::RunnerError> {
            panic!("slash command must skip the LLM runner");
        }
    }

    fn build_test_gateway(tmp: &tempfile::TempDir) -> Gateway {
        let cfg = Config::default(); // or whatever the test convention is
        let data_dir = tmp.path().to_str().unwrap();
        Gateway::new(cfg, data_dir, Box::new(PanicRunner), Arc::new(StubFacts))
    }

    #[tokio::test]
    async fn slash_help_returns_deterministic_string_without_llm() {
        let tmp = tempfile::tempdir().unwrap();
        let gw = build_test_gateway(&tmp);
        let out = gw.chat("test-chat", "/help", "test").await.unwrap();
        assert!(out.contains("/help"));
        assert!(out.contains("/status"));
    }

    #[tokio::test]
    async fn unknown_slash_falls_through_to_llm() {
        let tmp = tempfile::tempdir().unwrap();
        let gw = build_test_gateway(&tmp);
        let res = gw.chat("test-chat", "/foo bar", "test").await;
        // PanicRunner panics — so an Err is fine. The point: slash interception
        // does NOT match `/foo`, and falls through to the runner.
        assert!(res.is_err() || res.unwrap_err().to_string().contains("LLM"));
    }
}
```

If `Config::default()` doesn't exist, find an existing pattern that builds a minimal Config in tests (e.g., search `Config {` in the existing codebase) and mirror it. There may already be a `Config::test_default()` or similar helper.

- [ ] **Step 2: Run, confirm fail (or panic)**

Run: `cd agent && cargo test --features desktop --no-default-features --lib gateway::tests::slash_ 2>&1 | tail -10`
Expected: fails — slash interception isn't wired up yet, so `chat()` will reach the LLM runner.

- [ ] **Step 3: Wire interception into `chat_with_events`**

Find the start of `chat_with_events` (`gateway.rs:134`). Insert this block immediately *after* the active-chat cancellation flag setup (after the `let cancel = { ... };` block at lines 142–150) and *before* the `info!("GW chat: ...")` line at 152:

```rust
        // Slash-command interception. Recognized commands skip the LLM
        // entirely (and skip auto-compaction below). Unknown `/foo`
        // falls through to the normal LLM path.
        if let Some(cmd) = crate::core::commands::parse(message) {
            let facts = self.runtime_facts(chat_id);
            let reply = crate::core::commands::execute(
                cmd,
                chat_id,
                &facts,
                self.sessions.as_ref(),
                self.cloud_store.as_deref().map(|s| s as &dyn _),
            ).await;

            if let Some(sender) = events {
                use crate::core::chat_events::ChatEvent;
                let _ = sender.send(ChatEvent::AssistantText { text: reply.clone() });
                let _ = sender.send(ChatEvent::Done);
            }

            // Drop the cancellation flag we just registered — no async work pending.
            self.active_chats.lock().unwrap().remove(chat_id);
            let _ = cancel; // suppress unused

            return Ok(reply);
        }
```

The exact `ChatEvent` variants might differ (look at `agent/src/core/chat_events.rs`). The general shape: emit one `assistant_text`-style event with the reply text, then `done`. Adapt names to what's actually defined.

The `cloud_store.as_deref().map(...)` cast pattern needs verification — `Arc<dyn ObjectStore>` borrows as `&dyn ObjectStore` directly. Simpler form likely: `self.cloud_store.as_ref().map(|s| s.as_ref())`.

- [ ] **Step 4: Run, confirm pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib gateway::tests::slash_ 2>&1 | tail -10`
Expected: `test result: ok. 2 passed`.

Also run all tests:
```bash
cd agent && cargo test --features desktop --no-default-features --lib 2>&1 | tail -5
```
Expected: no regressions.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/gateway.rs
git commit -m "feat(gateway): intercept slash commands at top of chat_with_events"
```

---

### Task 10: Add `Poller::set_my_commands`

**Files:**
- Modify: `agent/src/core/channels/telegram.rs`

- [ ] **Step 1: Write the failing test**

Find the existing `mod tests` block (search for `async fn channel_send_typing_posts_chataction`). Add inside, after the existing tests:

```rust
    #[tokio::test]
    async fn set_my_commands_posts_correct_url_and_body() {
        let http = MockHttpClient::new();
        http.push_response(200, r#"{"ok":true,"result":true}"#);

        let p = Poller::new("TOKEN".to_string());
        let cmds = &[
            ("new",    "Start a fresh chat"),
            ("clear",  "Wipe history"),
        ];
        p.set_my_commands(&http, cmds).await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert!(reqs[0].url.contains("/setMyCommands"),
            "URL was: {}", reqs[0].url);

        let body = parse_body_json(&reqs[0]);
        let arr = body["commands"].as_array().expect("commands array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["command"], "new");
        assert_eq!(arr[0]["description"], "Start a fresh chat");
    }

    #[tokio::test]
    async fn set_my_commands_non_200_errors() {
        let http = MockHttpClient::new();
        http.push_response(429, r#"{"ok":false,"description":"Too Many Requests"}"#);

        let p = Poller::new("TOKEN".to_string());
        let result = p.set_my_commands(&http, &[("ping", "Test")]).await;
        assert!(result.is_err(), "non-200 should bubble as Err");
    }
```

- [ ] **Step 2: Run, confirm fail**

Run: `cd agent && cargo test --features desktop --no-default-features --lib channels::telegram::tests::set_my_commands 2>&1 | tail -5`
Expected: compile error, `set_my_commands` not found.

- [ ] **Step 3: Implement `set_my_commands`**

Find `impl Poller` (around line 32 of telegram.rs). Add this method:

```rust
    /// Register the bot's command list with Telegram.
    ///
    /// Idempotent — calling on every boot with the same payload is fine.
    /// Failures are non-fatal: caller should log and continue.
    pub async fn set_my_commands(
        &self,
        http: &dyn HttpClient,
        commands: &[(&str, &str)],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/setMyCommands",
            self.bot_token,
        );
        let payload = serde_json::json!({
            "commands": commands
                .iter()
                .map(|(name, desc)| serde_json::json!({
                    "command": name,
                    "description": desc,
                }))
                .collect::<Vec<_>>(),
        });
        let body = serde_json::to_vec(&payload)?;

        let mut headers = Headers::new();
        headers.insert("Content-Type", "application/json");

        let resp = http.post(&url, &headers, &body).await?;
        if resp.status != 200 {
            return Err(format!(
                "setMyCommands returned status {}",
                resp.status,
            ).into());
        }
        Ok(())
    }
```

The `bot_token` field name and `Headers::insert` API may be slightly different — check the existing `deliver` and `send_typing` methods on the `TelegramChannel` (around line 132+ of `telegram.rs`) for the right idiom.

- [ ] **Step 4: Run, confirm pass**

Run: `cd agent && cargo test --features desktop --no-default-features --lib channels::telegram:: 2>&1 | tail -5`
Expected: all telegram tests pass, including the two new ones.

- [ ] **Step 5: Commit**

```bash
git add agent/src/core/channels/telegram.rs
git commit -m "feat(telegram): Poller::set_my_commands for code-owned BotFather menu"
```

---

### Task 11: Wire `set_my_commands` into desktop boot + add `DesktopHostFacts`

**Files:**
- Create: `agent/src/desktop/host_facts.rs`
- Modify: `agent/src/desktop/mod.rs`
- Modify: `agent/src/desktop/run.rs`

- [ ] **Step 1: Create `agent/src/desktop/host_facts.rs`**

```rust
//! `HostFacts` impl for the desktop build.
//!
//! Heap fields are `None` (no equivalent of `esp_get_free_heap_size`
//! that's worth wiring up for desktop dev). Link is always `Desktop`.
//! Hostname comes from the `hostname` crate (already a desktop dep).

use crate::core::commands::{HostFacts, LinkKind};
use std::time::Instant;

pub struct DesktopHostFacts {
    started: Instant,
    hostname: String,
}

impl DesktopHostFacts {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            hostname: hostname::get()
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| "desktop".to_string()),
        }
    }
}

impl Default for DesktopHostFacts {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFacts for DesktopHostFacts {
    fn hostname(&self) -> String { self.hostname.clone() }
    fn ip(&self) -> Option<String> { None } // desktop dev — not exposing local IP
    fn link(&self) -> LinkKind { LinkKind::Desktop }
    fn free_internal_heap(&self) -> Option<u32> { None }
    fn free_psram(&self) -> Option<u32> { None }
    fn uptime_secs(&self) -> u64 { self.started.elapsed().as_secs() }
}
```

- [ ] **Step 2: Register the module**

Add `pub mod host_facts;` to `agent/src/desktop/mod.rs` next to other `pub mod` declarations.

- [ ] **Step 3: Wire `DesktopHostFacts` into `run.rs` Gateway construction**

In `agent/src/desktop/run.rs`, find where `Gateway::new` (or `new_with_cloud`) is called. Replace the `StubFacts` placeholder with:

```rust
let host_facts: std::sync::Arc<dyn crate::core::commands::HostFacts> =
    std::sync::Arc::new(crate::desktop::host_facts::DesktopHostFacts::new());
```

Pass that to the `Gateway` constructor.

- [ ] **Step 4: Wire `set_my_commands` into the Telegram poller boot**

In `spawn_telegram_loop` (around `desktop/run.rs:124`), find the `let producer_token = bot_token.clone();` line just before the inner `tokio::spawn`. Insert:

```rust
        // Register the BotFather menu — single source of truth in commands::menu().
        // Failures are non-fatal: if Telegram rate-limits or the bot can't
        // reach the API, the menu is degraded but the bot still works.
        let menu_token = bot_token.clone();
        let menu_http = http.clone();
        tokio::spawn(async move {
            let p = Poller::new(menu_token);
            if let Err(e) = p
                .set_my_commands(&*menu_http, crate::core::commands::menu())
                .await
            {
                tracing::warn!(error = %e, "setMyCommands failed (non-fatal)");
            }
        });
```

- [ ] **Step 5: Verify compile**

```bash
cd agent && cargo check --features desktop --no-default-features 2>&1 | tail -5
```
Expected: exit 0.

Also run all tests:
```bash
cd agent && cargo test --features desktop --no-default-features --lib 2>&1 | tail -5
```
Expected: no regressions.

- [ ] **Step 6: Manual desktop smoke (optional but recommended)**

```bash
cd agent && cargo run --features desktop --no-default-features
```

In another terminal:
```bash
curl -s http://localhost:8080/api/chat \
  -H 'Content-Type: application/json' \
  -d '{"message":"/help","chat_id":"smoke"}'
```
Expected: response body contains a markdown bullet list of all four commands.

```bash
curl -s http://localhost:8080/api/chat \
  -H 'Content-Type: application/json' \
  -d '{"message":"/status","chat_id":"smoke"}'
```
Expected: response body contains a markdown table with `Platform: linux` (or your host OS).

- [ ] **Step 7: Commit**

```bash
git add agent/src/desktop/host_facts.rs agent/src/desktop/mod.rs agent/src/desktop/run.rs
git commit -m "feat(desktop): add DesktopHostFacts and wire set_my_commands on poller boot"
```

---

### Task 12: Wire `set_my_commands` into ESP32 boot + add `Esp32HostFacts`

**Files:**
- Modify: `agent/src/main.rs`

- [ ] **Step 1: Add `Esp32HostFacts` impl**

Near the top of `main.rs` (after the `use` block), add:

```rust
#[cfg(feature = "esp32")]
struct Esp32HostFacts {
    hostname: String,
    started: std::time::Instant,
    nic: std::sync::Arc<dyn zenclaw_agent::net::Nic>,
}

#[cfg(feature = "esp32")]
impl Esp32HostFacts {
    fn new(
        hostname: String,
        nic: std::sync::Arc<dyn zenclaw_agent::net::Nic>,
    ) -> Self {
        Self {
            hostname,
            started: std::time::Instant::now(),
            nic,
        }
    }
}

#[cfg(feature = "esp32")]
impl zenclaw_agent::core::commands::HostFacts for Esp32HostFacts {
    fn hostname(&self) -> String { self.hostname.clone() }

    fn ip(&self) -> Option<String> {
        self.nic.ip_info().ok().map(|i| i.ip.to_string())
    }

    fn link(&self) -> zenclaw_agent::core::commands::LinkKind {
        // Distinguish WiFi from Ethernet by feature-gate.
        // RSSI from EspWifi is feature-gated to nic-wifi-internal.
        #[cfg(feature = "nic-wifi-internal")]
        {
            // SSID: from NVS or the Nic interface if it exposes that.
            // RSSI: read from esp_wifi_sta_get_ap_info if available.
            // For v1 — return SSID from `ip_info` extension if exposed,
            // else "?". RSSI = None if not directly readable.
            return zenclaw_agent::core::commands::LinkKind::Wifi {
                ssid: self.nic.link_label().unwrap_or_else(|| "?".to_string()),
                rssi: self.nic.rssi(),
            };
        }
        #[cfg(all(feature = "nic-eth", not(feature = "nic-wifi-internal")))]
        {
            return zenclaw_agent::core::commands::LinkKind::Ethernet;
        }
        #[allow(unreachable_code)]
        zenclaw_agent::core::commands::LinkKind::Ethernet
    }

    fn free_internal_heap(&self) -> Option<u32> {
        Some(unsafe { esp_idf_svc::sys::esp_get_free_heap_size() })
    }

    fn free_psram(&self) -> Option<u32> {
        // CAP_SPIRAM = 1 << 10 (BIT(10)); see `esp_heap_caps.h`. Use the
        // existing constant if exposed via esp_idf_svc::sys, else this raw value.
        const MALLOC_CAP_SPIRAM: u32 = 1 << 10;
        let v = unsafe {
            esp_idf_svc::sys::heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
        };
        // 0 likely means "no PSRAM" or "unsupported"; both → None.
        if v == 0 { None } else { Some(v as u32) }
    }

    fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}
```

If `Nic` doesn't expose `link_label()` and `rssi()` already, check the existing `/api/status` JSON-builder in `main.rs:620+` for how it gets these — pattern-match the call shape into our impl. Worst case: leave `ssid` as `"unknown"` and `rssi: None` for v1, document as follow-up.

- [ ] **Step 2: Build the `Esp32HostFacts` near where `Gateway` is constructed**

In `main.rs` around line 209 where `cloud_handles` is consumed, before the `Gateway::new`/`new_with_cloud` call, add:

```rust
let host_facts: std::sync::Arc<dyn zenclaw_agent::core::commands::HostFacts> =
    std::sync::Arc::new(Esp32HostFacts::new(
        hostname.clone(),
        nic.clone(),
    ));
```

Then pass `host_facts` to `Gateway::new` / `Gateway::new_with_cloud`. Remove the `StubFacts` placeholder from Task 8.

- [ ] **Step 3: Wire `set_my_commands` at the start of the Telegram thread**

In `main.rs` around line 1719 where the Telegram thread is set up (search for `Poller::new` and look for the loop that calls `poll_once`). Just before the loop starts, register the menu:

```rust
        // Register the BotFather menu (single source of truth in commands::menu()).
        // Non-fatal: if it fails (rate limit, no network), log and continue.
        if let Err(e) = esp_idf_svc::hal::task::block_on(
            poller.set_my_commands(&*http, zenclaw_agent::core::commands::menu()),
        ) {
            log::warn!("setMyCommands failed (non-fatal): {}", e);
        }
```

The `&*http` shape mirrors the existing `poll_once(&*http, 10)` usage at line 1719.

- [ ] **Step 4: Build for both supported boards**

```bash
cd agent && just build devkitc 2>&1 | tail -10
```
Expected: `Finished release` with no errors.

```bash
cd agent && just build guition-p4 2>&1 | tail -10
```
Expected: `Finished release` with no errors.

If P4 is broken because `nic-eth` feature path isn't exercised correctly in `Esp32HostFacts::link()`, fix the `cfg` gating until both build clean.

- [ ] **Step 5: Commit**

```bash
git add agent/src/main.rs
git commit -m "feat(esp32): Esp32HostFacts impl + wire set_my_commands on Telegram thread"
```

---

### Task 13: Build wizard firmware artifacts and manual smoke test

**Files:**
- (No code changes — this task rebuilds firmware artifacts and runs the end-to-end smoke.)

Per `feedback_wizard_firmware_rebuild.md`: the user flashes via the web wizard, not `just flash`. After every agent code change, the firmware artifacts the wizard serves need rebuilding.

- [ ] **Step 1: Rebuild wizard firmware**

```bash
./scripts/build-rust-firmware.sh devkitc
```
Expected: `firmware.json` regenerated, devkitc artifact updated.

If P4 is in scope for the user's testing too:
```bash
./scripts/build-rust-firmware.sh guition-p4
```

- [ ] **Step 2: Hand off to user for reflash**

Stop here. Tell the user:
> "Wizard firmware rebuilt. Please reflash via the web wizard, then run the smoke checklist below."

- [ ] **Step 3 (user-driven, post-reflash): Telegram smoke**

User runs each of these in Telegram chat with the bot:

1. `/help` → expect markdown bullet list with `/new`, `/clear`, `/status`, `/help`.
2. `/status` → expect markdown table with `Platform: esp32-s3` (NOT `unknown`), real hostname, real heap numbers, model name. Should look like:
   ```
   **<AgentName> Status**

   | Field | Value |
   |---|---|
   | Hostname | `zenclaw-...` |
   | IP | 192.168.x.y |
   | Link | WiFi yourssid (-XX dBm) |
   | Platform | `esp32-s3` |
   | Free internal heap | XX KB |
   | Free PSRAM | X MB |
   | ... |
   ```
3. Send some chat ("hello"), then `/clear`, then "ping" → the second "ping" reply should not reference "hello" (history wiped).
4. Open BotFather → `/mybots` → bot → `Edit Bot` → `Edit Commands` → expect the four commands listed exactly as in `commands::menu()`.

- [ ] **Step 4 (user-driven): Web smoke**

In the web UI (`http://<hostname>.local`), open Chat:

1. Send `/help` → markdown rendering of bullet list.
2. Send `/status` → markdown table renders correctly.
3. Send `/clear` → "Session cleared." Subsequent messages start with no prior context.

- [ ] **Step 5: Document any issues found**

If anything in the smoke fails, file as a follow-up task. If smoke passes, no further action.

- [ ] **Step 6: Final commit (if any cleanup needed)**

If the smoke surfaced minor issues (e.g., a description string change, em-dash rendering), commit:
```bash
git add -p
git commit -m "fix(commands): <description of smoke-test fix>"
```

---

## Self-Review Checklist (post-execution)

After all tasks complete, before opening a PR:

- [ ] All four commands work on both Telegram and web.
- [ ] BotFather menu (visible in Telegram client) matches `commands::menu()`.
- [ ] `/status` shows `Platform: esp32-s3` or `esp32-p4` (not `unknown`).
- [ ] `/clear` preserves `model_override` (verify by setting a model fast-dial, clearing, sending a new message — model used should be the override, not the config default).
- [ ] No regressions in existing 193 tests (`cargo test --features desktop --no-default-features --lib`).
- [ ] Both ESP32 boards build clean (`just build devkitc` + `just build guition-p4`).
- [ ] Wizard firmware artifact regenerated (`scripts/build-rust-firmware.sh`).
- [ ] No `// TODO` or `unimplemented!()` left in `commands.rs`.

## Out of Scope (tracked as v2 follow-ups)

- `GET /api/commands` endpoint serving `commands::menu()` as JSON for a future autocomplete dropdown in `chat.vue`.
- `/memory`, `/restart`, `/model <name>` commands.
- Confirmation prompts for `/clear`.
- Multi-session per channel (web chat picker).
- Localized command descriptions.
- Replicator delete-op (current S3 wipe in `clear_with_store` is direct-store, not via the replicator queue — fine for `/clear` rarity but worth unifying eventually).
