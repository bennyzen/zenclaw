# Telegram Path Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace ESP32's hand-rolled Telegram HTTP/JSON code and desktop's reqwest-based Telegram code with a single canonical implementation in `core/channels/telegram.rs`, sitting on the existing `Channel` and `HttpClient` traits.

**Architecture:** Two commits. Commit A is a no-behavior refactor that hoists the `Channel` trait into `core/`, deletes dead `CliChannel`, adds an `EspHttpClient` impl of `HttpClient`, and un-gates `pub mod platform` so ESP32 can reach the trait. Commit B adds `core/channels/telegram.rs` with `Poller` and `TelegramChannel`, then wires both `main.rs` (ESP32) and `desktop/run.rs` through it, deleting `desktop/telegram.rs` and the `tg_*` helpers in `main.rs`.

**Tech Stack:** Rust, `async_trait`, `esp-idf-svc` HTTP client (ESP32), `reqwest` (desktop), `serde_json`, `block_on` for ESP32 async-driving, `tokio` for desktop. Unit tests use a mock `HttpClient` impl in the same module — no hardware or network required.

**Reference spec:** `docs/superpowers/specs/2026-05-01-telegram-unification-design.md`

---

## File Structure

**New files:**
- `agent/src/core/channels/mod.rs` — `Channel` trait + `deliver_stream` default impl. (~30 lines.)
- `agent/src/core/channels/telegram.rs` — `Poller`, `IncomingMessage`, `TelegramChannel`, `impl Channel for TelegramChannel`, plus `MockHttpClient` and unit tests (cfg test). (~360 lines.)
- `agent/src/esp32/http_client.rs` — `EspHttpClient` struct + `impl HttpClient`. (~140 lines including stream_post.)

**Modified files:**
- `agent/src/lib.rs` — un-gate `pub mod platform`.
- `agent/src/core/mod.rs` — add `pub mod channels;`.
- `agent/src/esp32/mod.rs` — add `pub mod http_client;`.
- `agent/src/desktop/mod.rs` — remove `pub mod channels;` and `pub mod telegram;`.
- `agent/src/main.rs` — delete `tg_api`, `tg_http_get`, `tg_http_post` (lines 1260-1329); replace agent_thread Telegram block (~120 lines) with calls to `Poller::poll_once` + `TelegramChannel`; expand `agent_thread` signature; construct `Arc<EspHttpClient>` after NIC bring-up; honor `allowed_chat_ids`.
- `agent/src/desktop/run.rs` — construct `Arc<ReqwestHttpClient>`; rewrite `spawn_telegram_loop` against `Poller::poll_once` + `TelegramChannel`.

**Deleted files:**
- `agent/src/desktop/channels/mod.rs` (and `agent/src/desktop/channels/` directory).
- `agent/src/desktop/telegram.rs`.

---

## Task 1: Capture pre-change DevKitC baseline

**Files:**
- Modify (record numbers into): `docs/superpowers/specs/2026-05-01-telegram-unification-design.md` (the "Pre-change baseline" subsection of "Verification protocol").

This task is **manual hardware work**. It must run on `main` HEAD (commit `82b8606` — the spec commit) before any code changes. Skipping it loses the reference numbers Commit B is graded against.

- [ ] **Step 1: Confirm prerequisites**

Hardware: a DevKitC flashed-or-flashable. A working Telegram bot token in your `config.json` (or NVS) on the device. Wifi credentials present.

Run: `ls -la /dev/ttyACM0 || ls -la /dev/ttyUSB0`
Expected: at least one of them lists a device file.

- [ ] **Step 2: Build and flash on `main`**

Run from repo root:
```bash
cd agent
just build devkitc
just flash devkitc /dev/ttyACM0
```
Expected: `flash` finishes with "Resetting target device". Detach and let the board boot for ~12 seconds (WiFi connect + HTTP server up).

- [ ] **Step 3: Capture initial heap**

Substitute your hostname (the firmware logs it on the serial console — `mDNS: <name>.local`).

```bash
HOST=zenclaw-XXX.local
curl -sf "http://$HOST/api/status" | jq '.memory'
```
Expected: a JSON object like `{"free_kb": NNN, "total_kb": NNN, "used_kb": NNN}`. Record the three values as **`pre_boot`**.

- [ ] **Step 4: Trigger one Telegram round-trip**

From a Telegram client, send the message `ping` to your bot. Wait for the reply (≤30s). Verify the reply arrives.

- [ ] **Step 5: Capture post-roundtrip heap**

```bash
curl -sf "http://$HOST/api/status" | jq '.memory'
```
Record as **`pre_after_roundtrip`**.

- [ ] **Step 6: Record numbers in the spec**

Edit `docs/superpowers/specs/2026-05-01-telegram-unification-design.md`. Find the "Pre-change baseline" subsection. Replace its prose with a filled-in table:

```markdown
**Pre-change baseline (recorded on commit 82b8606, DevKitC, $(date +%Y-%m-%d)):**

| Metric         | Value      |
|----------------|------------|
| free_kb at boot       | <pre_boot.free_kb>   |
| used_kb at boot       | <pre_boot.used_kb>   |
| free_kb after 1 round-trip | <pre_after_roundtrip.free_kb> |
| used_kb after 1 round-trip | <pre_after_roundtrip.used_kb> |

Telegram round-trip: confirmed working on baseline.
```

- [ ] **Step 7: Commit the baseline numbers**

```bash
git add docs/superpowers/specs/2026-05-01-telegram-unification-design.md
git commit -m "docs: record pre-change DevKitC baseline for telegram unification"
```

---

## Task 2: Hoist `Channel` trait into `core/`, delete dead `CliChannel`

**Files:**
- Create: `agent/src/core/channels/mod.rs`
- Modify: `agent/src/core/mod.rs`
- Modify: `agent/src/lib.rs`
- Modify: `agent/src/desktop/mod.rs`
- Delete: `agent/src/desktop/channels/mod.rs` (and the empty `agent/src/desktop/channels/` directory)

This task has no behavior change — it moves a file, drops dead code, and un-gates the `platform` module so ESP32 can see the `HttpClient` trait.

- [ ] **Step 1: Create `core/channels/mod.rs`**

Write to `agent/src/core/channels/mod.rs`:

```rust
//! Delivery channels for messages produced by the gateway.
//!
//! `Channel` is the trait every output sink implements. Today only
//! `TelegramChannel` (in `core/channels/telegram.rs`) is wired; future
//! sinks (Slack, Matrix, web push) drop in here without touching gateway.

use async_trait::async_trait;

#[async_trait]
pub trait Channel: Send + Sync {
    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Default implementation routes through `deliver`. Override for true
    /// streaming (e.g. Telegram editMessageText with debounce).
    async fn deliver_stream(
        &self,
        chat_id: &str,
        chunk: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.deliver(chat_id, chunk).await
    }
}

pub mod telegram;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Channel impl that captures every call for assertion in tests.
    struct CapturingChannel {
        delivered: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl Channel for CapturingChannel {
        async fn deliver(
            &self,
            chat_id: &str,
            text: &str,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.delivered
                .lock()
                .unwrap()
                .push((chat_id.to_string(), text.to_string()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn deliver_stream_default_routes_to_deliver() {
        let ch = CapturingChannel {
            delivered: Mutex::new(Vec::new()),
        };
        ch.deliver_stream("chat42", "hello").await.unwrap();
        let recorded = ch.delivered.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "chat42");
        assert_eq!(recorded[0].1, "hello");
    }
}
```

(The `pub mod telegram;` line will fail to resolve until Task 5 creates the file. We'll address that by creating an empty `telegram.rs` shell here too.)

- [ ] **Step 2: Create empty `core/channels/telegram.rs` shell**

Write to `agent/src/core/channels/telegram.rs` (just enough so `pub mod telegram` in `mod.rs` resolves; will be filled in Task 5):

```rust
//! Telegram bot integration — Poller (long-poll receiver) and TelegramChannel
//! (sender, impl Channel). Both go through `&dyn HttpClient` so they work
//! identically on ESP32 and desktop.
//!
//! TODO Task 5: implement.
```

- [ ] **Step 3: Wire `core/channels` into `core/mod.rs`**

Modify `agent/src/core/mod.rs`. Add `pub mod channels;` to the alphabetical position:

```rust
pub mod agent_loop;
pub mod channels;
pub mod cloud;
pub mod compaction;
pub mod cron;
pub mod gateway;
pub mod prompt;
pub mod sessions;
pub mod tool_loop;
pub mod tools;
pub mod types;
pub mod workspace;

pub mod runner;
```

- [ ] **Step 4: Un-gate `pub mod platform` in `lib.rs`**

Modify `agent/src/lib.rs`. Change line 13-14 from:

```rust
#[cfg(feature = "desktop")]
pub mod platform;
```

to:

```rust
pub mod platform;
```

(The submodules `http_client`, `http_server`, `runtime` are pure trait definitions with no platform-specific deps — verified in Section 2 design phase.)

- [ ] **Step 5: Remove `desktop/channels` from `desktop/mod.rs`**

Modify `agent/src/desktop/mod.rs`. Delete these two lines:

```rust
#[cfg(feature = "desktop")]
pub mod channels;
```

The remaining `desktop/mod.rs` should still have `background`, `subagents`, `telegram` — leave those untouched (telegram gets removed in Task 11).

- [ ] **Step 6: Delete `desktop/channels/`**

```bash
rm -r agent/src/desktop/channels
```

- [ ] **Step 7: Verify desktop build**

```bash
cd agent
cargo build --no-default-features --features desktop 2>&1 | tail -20
```
Expected: `Finished ...` (success). No errors. `unused` warnings on `core::channels::Channel` are acceptable — there are no callers yet.

- [ ] **Step 8: Verify ESP32 build**

```bash
cd agent
just build devkitc 2>&1 | tail -20
```
Expected: success. Build cache may take a few minutes if cold.

- [ ] **Step 9: Run the new test**

```bash
cd agent
cargo test --no-default-features --features desktop --lib core::channels::tests:: 2>&1 | tail -10
```
Expected: `test result: ok. 1 passed`.

- [ ] **Step 10: Stage and verify status**

```bash
git status
git add -A
git status
```
Expected staged changes: created `agent/src/core/channels/mod.rs`, `agent/src/core/channels/telegram.rs`; modified `agent/src/core/mod.rs`, `agent/src/lib.rs`, `agent/src/desktop/mod.rs`; deleted `agent/src/desktop/channels/mod.rs`.

(Do not commit yet — Task 3 lands in the same commit.)

---

## Task 3: Add `EspHttpClient` (HttpClient impl for ESP32)

**Files:**
- Create: `agent/src/esp32/http_client.rs`
- Modify: `agent/src/esp32/mod.rs`

No callers yet — the new struct just needs to compile and pass type-check. Wire-up happens in Task 13.

- [ ] **Step 1: Write `esp32/http_client.rs`**

Write to `agent/src/esp32/http_client.rs`:

```rust
//! HttpClient implementation backed by esp-idf-svc's blocking HTTP client
//! with TLS via the bundled certificate store.
//!
//! Internally takes `crate::TLS_MUTEX` per call — the device can only sustain
//! one mbedTLS context at a time, so concurrent HTTPS calls would otherwise
//! corrupt each other. Held for the full duration of one request (handshake
//! + body). On poison, recovers via `into_inner` since there's no way to
//! reset mbedTLS without rebooting the chip.
//!
//! The `async fn` bodies execute synchronously when polled — ESP32 has no
//! executor that yields on I/O. This is the same pattern used in
//! `esp32/runner.rs` and works under `block_on`.

use async_trait::async_trait;
use std::time::Duration;

use crate::platform::http_client::{Headers, HttpClient, Response};

pub struct EspHttpClient {
    timeout: Duration,
}

impl EspHttpClient {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

impl Default for EspHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
impl HttpClient for EspHttpClient {
    async fn get(&self, url: &str, headers: &Headers) -> Result<Response, BoxErr> {
        execute(self, esp_idf_svc::http::Method::Get, url, headers, None)
    }

    async fn post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, BoxErr> {
        execute(
            self,
            esp_idf_svc::http::Method::Post,
            url,
            headers,
            Some(body),
        )
    }

    async fn put(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<Response, BoxErr> {
        execute(
            self,
            esp_idf_svc::http::Method::Put,
            url,
            headers,
            Some(body),
        )
    }

    async fn delete(&self, url: &str, headers: &Headers) -> Result<Response, BoxErr> {
        execute(self, esp_idf_svc::http::Method::Delete, url, headers, None)
    }

    async fn stream_post(
        &self,
        url: &str,
        headers: &Headers,
        body: &[u8],
        mut on_chunk: Box<dyn FnMut(String) + Send>,
    ) -> Result<(), BoxErr> {
        use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

        let _tls_guard = crate::TLS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let config = HttpConfig {
            buffer_size: Some(1024),
            buffer_size_tx: Some(1024),
            timeout: Some(self.timeout),
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let mut conn =
            EspHttpConnection::new(&config).map_err(|e| format!("HTTP init: {}", e))?;

        let body_len = body.len().to_string();
        let mut header_pairs: Vec<(&str, &str)> = Vec::new();
        header_pairs.push(("Content-Length", body_len.as_str()));
        for (k, v) in headers {
            header_pairs.push((k.as_str(), v.as_str()));
        }

        conn.initiate_request(esp_idf_svc::http::Method::Post, url, &header_pairs)
            .map_err(|e| format!("req: {}", e))?;
        conn.write_all(body).map_err(|e| format!("write: {}", e))?;
        conn.initiate_response()
            .map_err(|e| format!("resp: {}", e))?;

        let mut buf = [0u8; 2048];
        loop {
            let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
            if n == 0 {
                break;
            }
            // Best-effort UTF-8 decode of this chunk; SSE/text streams should be valid.
            let s = std::str::from_utf8(&buf[..n])
                .map(String::from)
                .unwrap_or_else(|_| String::from_utf8_lossy(&buf[..n]).into_owned());
            on_chunk(s);
        }
        Ok(())
    }
}

fn execute(
    client: &EspHttpClient,
    method: esp_idf_svc::http::Method,
    url: &str,
    headers: &Headers,
    body: Option<&[u8]>,
) -> Result<Response, BoxErr> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

    let _tls_guard = crate::TLS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    let config = HttpConfig {
        buffer_size: Some(1024),
        buffer_size_tx: Some(1024),
        timeout: Some(client.timeout),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut conn =
        EspHttpConnection::new(&config).map_err(|e| format!("HTTP init: {}", e))?;

    let body_len_str;
    let mut header_pairs: Vec<(&str, &str)> = Vec::new();
    if let Some(b) = body {
        body_len_str = b.len().to_string();
        header_pairs.push(("Content-Length", body_len_str.as_str()));
    }
    for (k, v) in headers {
        header_pairs.push((k.as_str(), v.as_str()));
    }

    conn.initiate_request(method, url, &header_pairs)
        .map_err(|e| format!("req: {}", e))?;

    if let Some(b) = body {
        conn.write_all(b).map_err(|e| format!("write: {}", e))?;
    }

    conn.initiate_response()
        .map_err(|e| format!("resp: {}", e))?;

    let status = conn.status();
    let mut resp_body = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 {
            break;
        }
        resp_body.extend_from_slice(&buf[..n]);
    }

    Ok(Response {
        status,
        body: resp_body,
        headers: Headers::new(),
    })
}
```

- [ ] **Step 2: Wire into `esp32/mod.rs`**

Modify `agent/src/esp32/mod.rs`. Result:

```rust
pub mod http_client;
pub mod runner;
```

- [ ] **Step 3: Verify ESP32 build**

```bash
cd agent
just build devkitc 2>&1 | tail -20
```
Expected: success. The new struct is unused, so an `unused` warning on `EspHttpClient` is acceptable.

- [ ] **Step 4: Verify desktop build still passes**

```bash
cd agent
cargo build --no-default-features --features desktop 2>&1 | tail -10
```
Expected: success. (`esp32/http_client.rs` is `#[cfg(feature = "esp32")]`-gated by virtue of being inside `esp32/`, which is itself gated.)

- [ ] **Step 5: Stage**

```bash
git add agent/src/esp32/http_client.rs agent/src/esp32/mod.rs
git status
```

(Still no commit — Task 4 commits both Task 2 and Task 3 together.)

---

## Task 4: Commit A (refactor: hoist Channel + add EspHttpClient)

**Files:** none new. This task creates the commit.

- [ ] **Step 1: Verify all expected changes are staged**

```bash
git status
```
Expected staged:
- new file: `agent/src/core/channels/mod.rs`
- new file: `agent/src/core/channels/telegram.rs` (the empty shell)
- new file: `agent/src/esp32/http_client.rs`
- modified: `agent/src/core/mod.rs`
- modified: `agent/src/lib.rs`
- modified: `agent/src/desktop/mod.rs`
- modified: `agent/src/esp32/mod.rs`
- deleted: `agent/src/desktop/channels/mod.rs`

- [ ] **Step 2: Commit**

```bash
git commit -m "$(cat <<'EOF'
refactor: hoist Channel trait + add EspHttpClient

First commit of the Telegram path unification slice. Pure structural
prep — no behavior change on either platform; new code is unreachable
from existing call sites.

- Move Channel trait from desktop/channels/mod.rs into core/channels/mod.rs.
  Drop the #[cfg(feature = "desktop")] gate on the trait so ESP32 can
  reach it. Drop the ChannelKind enum and the kind() method (unused
  ceremony — single-variant after CliChannel removal). Give
  deliver_stream a default impl that calls deliver, so future Telegram
  editMessageText streaming can override without revising the trait.

- Delete CliChannel and the empty desktop/channels/ directory. Confirmed
  zero callers anywhere in agent/.

- Un-gate pub mod platform in lib.rs so ESP32 can see HttpClient. The
  http_server / runtime submodules are pure trait definitions with no
  platform-specific deps.

- Add esp32/http_client.rs with EspHttpClient impl HttpClient. Internally
  takes crate::TLS_MUTEX per call (poison-recovered via into_inner) and
  drives EspHttpConnection synchronously — same pattern as esp32/runner.rs.
  Unused for now; wired in the unification commit.

- Add empty agent/src/core/channels/telegram.rs shell so the pub mod
  resolves; filled in the unification commit.

Verification: cargo build --features desktop and just build devkitc both
succeed; cargo test --features desktop --lib passes the new
deliver_stream default-impl test.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3: Confirm commit**

```bash
git log --oneline -3
```
Expected: top entry is the new "refactor: hoist Channel trait + add EspHttpClient" commit.

---

## Task 5: Scaffold `core/channels/telegram.rs` with `MockHttpClient` + `Poller` skeleton

**Files:**
- Modify: `agent/src/core/channels/telegram.rs`

This task replaces the empty shell from Task 2 with a skeleton: types, struct definitions, and a unit-test infrastructure. No method bodies yet — the next task fills `Poller::poll_once`. We do this so types and `MockHttpClient` exist before the tests reference them.

- [ ] **Step 1: Replace the shell with scaffolding**

Overwrite `agent/src/core/channels/telegram.rs`:

```rust
//! Telegram bot integration — `Poller` (long-poll receiver) and
//! `TelegramChannel` (sender, impl `Channel`). Both go through
//! `&dyn HttpClient` so they work identically on ESP32 and desktop.
//!
//! Defaults:
//! - Long-poll timeout: caller-supplied; recommended 10s.
//! - parse_mode: None. LLM replies aren't sanitized for Markdown special
//!   chars (`_`, `*`, `[`, `` ` ``); a stray underscore returns 400 from
//!   Telegram. Opt in via `with_parse_mode(Some("Markdown"))` if you
//!   know your replies are safe.
//! - allowed_chat_ids: not enforced inside Poller — caller filters
//!   returned `IncomingMessage`s, since Poller doesn't see config.

use async_trait::async_trait;
use std::sync::Arc;

use crate::core::channels::Channel;
use crate::platform::http_client::{Headers, HttpClient};

#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub chat_id: String,
    pub text: String,
    pub from_username: Option<String>,
}

pub struct Poller {
    bot_token: String,
    offset: i64,
}

impl Poller {
    pub fn new(bot_token: String) -> Self {
        Self {
            bot_token,
            offset: 0,
        }
    }

    /// One getUpdates round-trip with `timeout_secs` long-poll.
    /// Advances internal offset; returns whatever arrived (possibly empty).
    /// Caller drives cadence (interleaved on ESP32, tokio loop on desktop).
    pub async fn poll_once(
        &mut self,
        _http: &dyn HttpClient,
        _timeout_secs: u32,
    ) -> Result<Vec<IncomingMessage>, Box<dyn std::error::Error + Send + Sync>> {
        // Filled in Task 7.
        unimplemented!("poll_once: filled in Task 7")
    }
}

pub struct TelegramChannel {
    bot_token: String,
    http: Arc<dyn HttpClient>,
    parse_mode: Option<String>,
}

impl TelegramChannel {
    pub fn new(bot_token: String, http: Arc<dyn HttpClient>) -> Self {
        Self {
            bot_token,
            http,
            parse_mode: None,
        }
    }

    pub fn with_parse_mode(mut self, mode: Option<String>) -> Self {
        self.parse_mode = mode;
        self
    }

    /// Telegram-specific (not on Channel trait — Cli has no notion of typing).
    pub async fn send_typing(
        &self,
        _chat_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Filled in Task 9.
        unimplemented!("send_typing: filled in Task 9")
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    async fn deliver(
        &self,
        _chat_id: &str,
        _text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Filled in Task 9.
        unimplemented!("deliver: filled in Task 9")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::http_client::Response;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Records every request and returns canned responses popped FIFO.
    pub(crate) struct MockHttpClient {
        canned: Mutex<VecDeque<Result<Response, String>>>,
        recorded: Mutex<Vec<RecordedRequest>>,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct RecordedRequest {
        pub method: &'static str,
        pub url: String,
        pub body: Vec<u8>,
    }

    impl MockHttpClient {
        pub fn new() -> Self {
            Self {
                canned: Mutex::new(VecDeque::new()),
                recorded: Mutex::new(Vec::new()),
            }
        }

        pub fn push_response(&self, status: u16, body: &str) {
            self.canned.lock().unwrap().push_back(Ok(Response {
                status,
                body: body.as_bytes().to_vec(),
                headers: Headers::new(),
            }));
        }

        pub fn push_error(&self, msg: &str) {
            self.canned.lock().unwrap().push_back(Err(msg.to_string()));
        }

        pub fn requests(&self) -> Vec<RecordedRequest> {
            self.recorded.lock().unwrap().clone()
        }

        fn next_response(&self) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            match self.canned.lock().unwrap().pop_front() {
                Some(Ok(r)) => Ok(r),
                Some(Err(e)) => Err(e.into()),
                None => Err("MockHttpClient: no canned response remaining".into()),
            }
        }
    }

    #[async_trait]
    impl HttpClient for MockHttpClient {
        async fn get(
            &self,
            url: &str,
            _headers: &Headers,
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "GET",
                url: url.to_string(),
                body: Vec::new(),
            });
            self.next_response()
        }

        async fn post(
            &self,
            url: &str,
            _headers: &Headers,
            body: &[u8],
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "POST",
                url: url.to_string(),
                body: body.to_vec(),
            });
            self.next_response()
        }

        async fn put(
            &self,
            url: &str,
            _headers: &Headers,
            body: &[u8],
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "PUT",
                url: url.to_string(),
                body: body.to_vec(),
            });
            self.next_response()
        }

        async fn delete(
            &self,
            url: &str,
            _headers: &Headers,
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "DELETE",
                url: url.to_string(),
                body: Vec::new(),
            });
            self.next_response()
        }

        async fn stream_post(
            &self,
            _url: &str,
            _headers: &Headers,
            _body: &[u8],
            _on_chunk: Box<dyn FnMut(String) + Send>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            unreachable!("stream_post not used in Telegram path")
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd agent
cargo build --no-default-features --features desktop 2>&1 | tail -10
```
Expected: success. Warnings about unused fields/struct are acceptable.

- [ ] **Step 3: Verify tests still compile**

```bash
cd agent
cargo test --no-default-features --features desktop --lib --no-run 2>&1 | tail -10
```
Expected: success.

---

## Task 6: Write `Poller` tests (TDD: failing first)

**Files:**
- Modify: `agent/src/core/channels/telegram.rs` (append to `tests` mod)

Add seven tests for `Poller::poll_once`. They will compile but panic at `unimplemented!()` — that's the expected "failing" state for TDD.

- [ ] **Step 1: Append `Poller` tests inside the `tests` mod**

Insert these test functions at the end of the existing `mod tests` block in `core/channels/telegram.rs` (just before the closing `}` of `mod tests`):

```rust
    // ───────── Poller tests ─────────

    #[tokio::test]
    async fn poll_empty_result_returns_empty_vec() {
        let http = MockHttpClient::new();
        http.push_response(200, r#"{"ok":true,"result":[]}"#);
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn poll_one_text_message_extracted() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[{
                "update_id": 100,
                "message": {
                    "chat": {"id": 42},
                    "text": "hello bot",
                    "from": {"username": "alice"}
                }
            }]}"#,
        );
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].chat_id, "42");
        assert_eq!(msgs[0].text, "hello bot");
        assert_eq!(msgs[0].from_username.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn poll_advances_offset() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[
                {"update_id": 100, "message": {"chat": {"id": 1}, "text": "a"}},
                {"update_id": 102, "message": {"chat": {"id": 1}, "text": "b"}}
            ]}"#,
        );
        http.push_response(200, r#"{"ok":true,"result":[]}"#);
        let mut p = Poller::new("TOKEN".to_string());
        let _ = p.poll_once(&http, 10).await.unwrap();
        // Second call should request offset=103.
        let _ = p.poll_once(&http, 10).await.unwrap();
        let urls: Vec<String> = http.requests().into_iter().map(|r| r.url).collect();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("offset=0"), "first url={}", urls[0]);
        assert!(urls[1].contains("offset=103"), "second url={}", urls[1]);
    }

    #[tokio::test]
    async fn poll_skips_update_without_message() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[
                {"update_id": 1, "callback_query": {"id": "x"}},
                {"update_id": 2, "message": {"chat": {"id": 9}, "text": "real"}}
            ]}"#,
        );
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "real");
    }

    #[tokio::test]
    async fn poll_skips_message_without_text() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[
                {"update_id": 1, "message": {"chat": {"id": 9}, "photo": []}},
                {"update_id": 2, "message": {"chat": {"id": 9}, "text": "hi"}}
            ]}"#,
        );
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hi");
    }

    #[tokio::test]
    async fn poll_malformed_json_errors() {
        let http = MockHttpClient::new();
        http.push_response(200, "not json at all{{{");
        let mut p = Poller::new("TOKEN".to_string());
        let result = p.poll_once(&http, 10).await;
        assert!(result.is_err(), "expected parse error, got {:?}", result);
    }

    #[tokio::test]
    async fn poll_non_200_errors() {
        let http = MockHttpClient::new();
        http.push_response(401, r#"{"ok":false,"description":"unauthorized"}"#);
        let mut p = Poller::new("BAD".to_string());
        let result = p.poll_once(&http, 10).await;
        assert!(result.is_err(), "expected 401 error, got {:?}", result);
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("401"), "error should mention status: {}", msg);
    }
```

- [ ] **Step 2: Run the tests — expect them to fail**

```bash
cd agent
cargo test --no-default-features --features desktop --lib core::channels::telegram::tests::poll_ 2>&1 | tail -20
```
Expected: 7 tests, all FAILING with `panicked at 'not yet implemented: poll_once: filled in Task 7'`.

This confirms the test wiring is correct — they're hitting the right method, just not the implementation.

---

## Task 7: Implement `Poller::poll_once`

**Files:**
- Modify: `agent/src/core/channels/telegram.rs` (replace `Poller::poll_once` body)

- [ ] **Step 1: Replace `poll_once` body**

In `core/channels/telegram.rs`, replace the existing `Poller::poll_once` (the `unimplemented!()` body) with:

```rust
    pub async fn poll_once(
        &mut self,
        http: &dyn HttpClient,
        timeout_secs: u32,
    ) -> Result<Vec<IncomingMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates?offset={}&timeout={}",
            self.bot_token, self.offset, timeout_secs
        );

        let resp = http.get(&url, &Headers::new()).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "Telegram getUpdates HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )
            .into());
        }

        let body: serde_json::Value = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("Telegram parse: {}", e))?;

        let updates = match body.get("result").and_then(|r| r.as_array()) {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let mut messages = Vec::new();
        for update in updates {
            if let Some(uid) = update.get("update_id").and_then(|v| v.as_i64()) {
                if uid >= self.offset {
                    self.offset = uid + 1;
                }
            }

            let msg = match update.get("message") {
                Some(m) => m,
                None => continue,
            };

            let chat_id = msg
                .get("chat")
                .and_then(|c| c.get("id"))
                .and_then(|id| id.as_i64())
                .map(|id| id.to_string());

            let text = msg.get("text").and_then(|t| t.as_str()).map(String::from);

            let from_username = msg
                .get("from")
                .and_then(|f| f.get("username"))
                .and_then(|u| u.as_str())
                .map(String::from);

            if let (Some(chat_id), Some(text)) = (chat_id, text) {
                messages.push(IncomingMessage {
                    chat_id,
                    text,
                    from_username,
                });
            }
        }

        Ok(messages)
    }
```

- [ ] **Step 2: Run the Poller tests**

```bash
cd agent
cargo test --no-default-features --features desktop --lib core::channels::telegram::tests::poll_ 2>&1 | tail -15
```
Expected: 7 passed, 0 failed.

If a test fails, read the failure carefully — it usually points to a JSON-shape assumption mismatch.

---

## Task 8: Write `TelegramChannel` tests (TDD: failing first)

**Files:**
- Modify: `agent/src/core/channels/telegram.rs` (append more tests)

- [ ] **Step 1: Append channel tests inside `tests` mod**

Add these test functions to the same `mod tests` block (after the Poller tests):

```rust
    // ───────── TelegramChannel tests ─────────

    fn parse_body_json(req: &RecordedRequest) -> serde_json::Value {
        serde_json::from_slice(&req.body)
            .unwrap_or_else(|e| panic!("body not JSON: {} ({:?})", e, req.body))
    }

    #[tokio::test]
    async fn channel_deliver_posts_sendmessage_with_chat_and_text() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("99", "hello").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert!(
            reqs[0].url.contains("/botTOKEN/sendMessage"),
            "url={}",
            reqs[0].url
        );
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["chat_id"], "99");
        assert_eq!(body["text"], "hello");
    }

    #[tokio::test]
    async fn channel_deliver_includes_parse_mode_when_set() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone())
            .with_parse_mode(Some("Markdown".to_string()));
        ch.deliver("1", "msg").await.unwrap();

        let reqs = http.requests();
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["parse_mode"], "Markdown");
    }

    #[tokio::test]
    async fn channel_deliver_omits_parse_mode_by_default() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", "msg").await.unwrap();

        let reqs = http.requests();
        let body = parse_body_json(&reqs[0]);
        assert!(
            body.get("parse_mode").is_none(),
            "parse_mode should be absent: {:?}",
            body
        );
    }

    #[tokio::test]
    async fn channel_deliver_non_200_errors() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(403, r#"{"ok":false,"description":"forbidden"}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        let result = ch.deliver("99", "blocked").await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("403"));
    }

    #[tokio::test]
    async fn channel_send_typing_posts_chataction() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.send_typing("99").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert!(
            reqs[0].url.contains("/botTOKEN/sendChatAction"),
            "url={}",
            reqs[0].url
        );
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["chat_id"], "99");
        assert_eq!(body["action"], "typing");
    }
```

- [ ] **Step 2: Run the channel tests — expect failures**

```bash
cd agent
cargo test --no-default-features --features desktop --lib core::channels::telegram::tests::channel_ 2>&1 | tail -15
```
Expected: 5 tests, all panicking on `unimplemented!("deliver: filled in Task 9")` or `unimplemented!("send_typing: filled in Task 9")`.

---

## Task 9: Implement `TelegramChannel::deliver` and `send_typing`

**Files:**
- Modify: `agent/src/core/channels/telegram.rs`

- [ ] **Step 1: Replace `send_typing` and `deliver` bodies**

In `core/channels/telegram.rs`, replace the two `unimplemented!()` method bodies:

```rust
    pub async fn send_typing(
        &self,
        chat_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendChatAction",
            self.bot_token
        );
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": "typing",
        })
        .to_string();
        let mut headers = Headers::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let resp = self.http.post(&url, &headers, body.as_bytes()).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "Telegram sendChatAction HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )
            .into());
        }
        Ok(())
    }
```

```rust
#[async_trait]
impl Channel for TelegramChannel {
    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );

        let mut payload = serde_json::Map::new();
        payload.insert(
            "chat_id".to_string(),
            serde_json::Value::String(chat_id.to_string()),
        );
        payload.insert(
            "text".to_string(),
            serde_json::Value::String(text.to_string()),
        );
        if let Some(mode) = &self.parse_mode {
            payload.insert(
                "parse_mode".to_string(),
                serde_json::Value::String(mode.clone()),
            );
        }
        let body = serde_json::Value::Object(payload).to_string();

        let mut headers = Headers::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let resp = self.http.post(&url, &headers, body.as_bytes()).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "Telegram sendMessage HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )
            .into());
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Run all telegram tests**

```bash
cd agent
cargo test --no-default-features --features desktop --lib core::channels::telegram::ts 2>&1 | tail -25
```
Expected: 12 tests passed (7 Poller + 5 channel), 0 failed.

- [ ] **Step 3: Run the full lib test suite (ensure nothing else broke)**

```bash
cd agent
cargo test --no-default-features --features desktop --lib 2>&1 | tail -15
```
Expected: all tests passed; new totals match `previous + 13` (12 telegram + 1 channel default-impl).

---

## Task 10: Wire desktop through new APIs; delete `desktop/telegram.rs`

**Files:**
- Modify: `agent/src/desktop/run.rs`
- Modify: `agent/src/desktop/mod.rs`
- Delete: `agent/src/desktop/telegram.rs`

- [ ] **Step 1: Rewrite `spawn_telegram_loop` and `run` in `desktop/run.rs`**

Replace the existing `desktop/run.rs` contents with:

```rust
use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::core::channels::telegram::{IncomingMessage, Poller, TelegramChannel};
use crate::core::gateway::Gateway;
use crate::core::runner::{LlmRunner, Runner};
use crate::desktop::background::BackgroundRunner;
use crate::desktop::http_client::ReqwestHttpClient;
use crate::platform::http_client::HttpClient;

use super::{start_api_server, AppState};

/// Desktop entry point.
///
/// Mirrors the embedded firmware's lifecycle on a host machine: loads
/// `config.json` from the current directory, constructs the same Gateway
/// the ESP32 firmware does, exposes the same HTTP API on `0.0.0.0:8080`,
/// optionally spawns the Telegram poller, and offers a stdin REPL when
/// stdout is a TTY (otherwise runs as a daemon until SIGINT).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config_path = "config.json".to_string();
    if !std::path::Path::new(&config_path).exists() {
        eprintln!(
            "Error: {} not found in current directory.\n\
             Create one with at least: agent_name, providers.default, and a provider entry with api_key + model.",
            config_path
        );
        std::process::exit(1);
    }
    let config = Config::load(&config_path)?;

    info!("ZenClaw Agent v{}", env!("CARGO_PKG_VERSION"));
    info!(
        "Agent: {}, Provider: {}",
        config.agent_name, config.providers.default
    );

    let data_dir = "data";
    std::fs::create_dir_all(format!("{}/sessions", data_dir))?;
    std::fs::create_dir_all(format!("{}/memory", data_dir))?;
    crate::core::workspace::seed_defaults(data_dir);

    let config_arc = Arc::new(config.clone());
    let runner: Box<dyn LlmRunner> = Box::new(Runner::new(config_arc));
    let gateway = Gateway::new(config.clone(), data_dir, runner);
    info!("Tools registered: {}", gateway.tools.len());

    let gateway = Arc::new(gateway);
    let http: Arc<dyn HttpClient> = Arc::new(ReqwestHttpClient::new());
    let start_time = Instant::now();

    let bg_cancel = CancellationToken::new();
    {
        let bg_gateway = gateway.clone();
        let bg_token = bg_cancel.clone();
        tokio::spawn(async move {
            let runner = BackgroundRunner::new(
                bg_gateway.config.clone(),
                bg_gateway.data_dir.clone(),
            );
            runner.run(bg_token).await;
        });
    }

    if let Some(ref tg) = config.channels.telegram {
        if tg.enabled && !tg.bot_token.is_empty() {
            spawn_telegram_loop(
                gateway.clone(),
                http.clone(),
                tg.bot_token.clone(),
                tg.allowed_chat_ids.clone(),
            );
            info!("Telegram poller started");
        }
    }

    let api_port: u16 = 8080;
    {
        let app_state = AppState {
            gateway: gateway.clone(),
            start_time,
            config_path: config_path.clone(),
        };
        tokio::spawn(async move {
            start_api_server(app_state, api_port).await;
        });
    }

    info!(
        "Ready. API on :{} — type a message, /quit to exit.",
        api_port
    );

    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        repl_loop(&gateway).await?;
    } else {
        info!("No TTY detected — running in daemon mode. SIGINT to stop.");
        tokio::signal::ctrl_c().await?;
    }

    bg_cancel.cancel();
    info!("Shutting down");
    Ok(())
}

fn spawn_telegram_loop(
    gateway: Arc<Gateway>,
    http: Arc<dyn HttpClient>,
    bot_token: String,
    allowed: Option<Vec<String>>,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<IncomingMessage>(32);

        // Producer: poll_once in a loop, send messages into channel.
        let producer_http = http.clone();
        let producer_token = bot_token.clone();
        tokio::spawn(async move {
            let mut poller = Poller::new(producer_token);
            loop {
                match poller.poll_once(&*producer_http, 10).await {
                    Ok(msgs) => {
                        for msg in msgs {
                            if tx.send(msg).await.is_err() {
                                tracing::info!("Poller channel closed, stopping");
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Telegram poll error, retrying in 5s");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        // Consumer: handle each message via gateway + telegram channel.
        let channel = TelegramChannel::new(bot_token, http.clone());

        while let Some(msg) = rx.recv().await {
            if let Some(ref ids) = allowed {
                if !ids.contains(&msg.chat_id) {
                    warn!(chat_id = %msg.chat_id, "Telegram message from disallowed chat");
                    continue;
                }
            }

            let gw = gateway.clone();
            let chat_id = msg.chat_id.clone();
            let text = msg.text.clone();
            // `channel` is cheap to clone-by-reference but TelegramChannel doesn't
            // impl Clone — instead, hold an Arc to share across spawned tasks.
            // Spawn so we can keep polling while a turn runs.
            let channel_for_task = TelegramChannel::new(
                channel.bot_token_for_share(),
                http.clone(),
            );
            // (parse_mode default; if you want Markdown wrap with .with_parse_mode())

            tokio::spawn(async move {
                if let Err(e) = channel_for_task.send_typing(&chat_id).await {
                    warn!(error = %e, chat_id = %chat_id, "send_typing failed");
                }
                match gw.chat(&chat_id, &text, "telegram").await {
                    Ok(reply) => {
                        if let Err(e) = channel_for_task.deliver(&chat_id, &reply).await {
                            error!(error = %e, chat_id = %chat_id, "Telegram deliver failed");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, chat_id = %chat_id, "Telegram chat error");
                        let _ = channel_for_task
                            .deliver(&chat_id, &format!("Error: {}", e))
                            .await;
                    }
                }
            });
        }
    });
}

async fn repl_loop(gateway: &Arc<Gateway>) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        print!("> ");
        std::io::stdout().flush()?;

        match lines.next_line().await? {
            Some(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "/quit" || line == "/exit" {
                    break;
                }
                match gateway.chat("cli", line, "cli").await {
                    Ok(response) => println!("\n{}\n", response),
                    Err(e) => eprintln!("\nError: {}\n", e),
                }
            }
            None => break,
        }
    }
    Ok(())
}
```

Note: the `bot_token_for_share()` helper above is **not** in `TelegramChannel` yet — it's a placeholder. The cleaner fix is to have `spawn_telegram_loop` capture `bot_token: String` directly into each spawned task instead of going through `channel.bot_token_for_share()`. Replace those two lines:

```rust
            let channel_for_task = TelegramChannel::new(
                channel.bot_token_for_share(),
                http.clone(),
            );
```

with:

```rust
            let channel_for_task =
                TelegramChannel::new(bot_token_for_share.clone(), http.clone());
```

and add `let bot_token_for_share = channel /* not needed */;` — actually the simplest path: clone `bot_token` into a binding before the loop and use it directly:

Insert just before `while let Some(msg) = rx.recv().await {`:
```rust
        // Held alongside `channel` so each spawned task can construct its own.
        let bot_token_for_share = channel.bot_token().to_string();
        // (Sentinel: TelegramChannel needs a `bot_token() -> &str` accessor.)
```

…and then add a getter in `core/channels/telegram.rs::TelegramChannel`:

```rust
    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }
```

(Add right after `with_parse_mode` definition.)

**Net result of this step:** rewrite `desktop/run.rs` per the body above; add the `bot_token()` accessor to `TelegramChannel`. Adjust `channel_for_task` construction to use `bot_token_for_share.clone()`.

- [ ] **Step 2: Add the `bot_token()` accessor to `TelegramChannel`**

In `core/channels/telegram.rs`, after `with_parse_mode`:

```rust
    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }
```

- [ ] **Step 3: Final form of `spawn_telegram_loop` consumer block**

Use this exact consumer block in `desktop/run.rs::spawn_telegram_loop` (replacing the placeholder construction noted above):

```rust
        let channel = TelegramChannel::new(bot_token.clone(), http.clone());
        let bot_token_for_tasks = bot_token.clone();

        while let Some(msg) = rx.recv().await {
            if let Some(ref ids) = allowed {
                if !ids.contains(&msg.chat_id) {
                    warn!(chat_id = %msg.chat_id, "Telegram message from disallowed chat");
                    continue;
                }
            }

            let gw = gateway.clone();
            let chat_id = msg.chat_id.clone();
            let text = msg.text.clone();
            let token_for_task = bot_token_for_tasks.clone();
            let http_for_task = http.clone();

            tokio::spawn(async move {
                let channel_for_task = TelegramChannel::new(token_for_task, http_for_task);

                if let Err(e) = channel_for_task.send_typing(&chat_id).await {
                    warn!(error = %e, chat_id = %chat_id, "send_typing failed");
                }
                match gw.chat(&chat_id, &text, "telegram").await {
                    Ok(reply) => {
                        if let Err(e) = channel_for_task.deliver(&chat_id, &reply).await {
                            error!(error = %e, chat_id = %chat_id, "Telegram deliver failed");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, chat_id = %chat_id, "Telegram chat error");
                        let _ = channel_for_task
                            .deliver(&chat_id, &format!("Error: {}", e))
                            .await;
                    }
                }
            });
        }

        // `channel` (the outer-scope one) and `bot_token_for_tasks` go out of
        // scope when the consumer loop ends. They're kept around solely to
        // confirm the producer task can still reference `bot_token` for new
        // poll cycles via its own clone.
        drop(channel);
```

(The outer `channel` variable is now mostly unused — keeping it kept the diff minimal in earlier drafting, but if Clippy complains, replace `let channel = TelegramChannel::new(...)` and the trailing `drop(channel)` with just letting `bot_token_for_tasks = bot_token.clone();` carry the share.)

- [ ] **Step 4: Remove `desktop/telegram.rs` from `desktop/mod.rs`**

In `agent/src/desktop/mod.rs`, delete these two lines:

```rust
#[cfg(feature = "desktop")]
pub mod telegram;
```

- [ ] **Step 5: Delete `desktop/telegram.rs`**

```bash
rm agent/src/desktop/telegram.rs
```

- [ ] **Step 6: Build and run tests**

```bash
cd agent
cargo build --no-default-features --features desktop 2>&1 | tail -15
```
Expected: success. Warnings about unused imports may appear — fix any that fail the build.

```bash
cargo test --no-default-features --features desktop --lib 2>&1 | tail -15
```
Expected: all tests pass.

---

## Task 11: Manual desktop verification — Telegram round-trip

**Files:** none changed.

- [ ] **Step 1: Confirm `config.json` has Telegram credentials**

```bash
cat agent/config.json | jq '.channels.telegram'
```
Expected: shows `enabled: true`, a non-empty `bot_token`, and (optionally) `default_chat_id` / `allowed_chat_ids`.

- [ ] **Step 2: Run desktop**

```bash
cd agent
cargo run --no-default-features --features desktop 2>&1 | tee /tmp/desktop-tg.log &
DESKTOP_PID=$!
sleep 5
```
Expected: log contains `Telegram poller started`.

- [ ] **Step 3: Send a Telegram round-trip**

From a Telegram client (allowed by `allowed_chat_ids` if set), send `ping` to your bot. Wait ≤30s for the reply.

Expected: a reply arrives. The desktop log should contain `Received Telegram message` and a successful `gateway.chat` followed by no `Telegram deliver failed` lines.

- [ ] **Step 4: Stop desktop**

```bash
kill $DESKTOP_PID
wait $DESKTOP_PID 2>/dev/null
```

If the round-trip failed, fix before proceeding to ESP32. Common pitfalls: missing Content-Type header, body shape mismatch, wrong URL.

---

## Task 12: Wire ESP32 `main.rs` through new APIs; delete `tg_*` helpers

**Files:**
- Modify: `agent/src/main.rs`

This is the largest single edit. Plan: (1) construct `Arc<EspHttpClient>` after NIC bring-up; (2) construct optional `Poller` + `TelegramChannel` next to the existing `bot_token` extraction; (3) expand `agent_thread` signature to accept those; (4) replace inline Telegram block in `agent_thread`; (5) delete `tg_api`, `tg_http_get`, `tg_http_post`.

- [ ] **Step 1: Add new imports near the top of `main.rs`**

Find the existing `#[cfg(feature = "esp32")]` import block (or near `use` statements at module top). Add:

```rust
#[cfg(feature = "esp32")]
use std::sync::Arc;
#[cfg(feature = "esp32")]
use zenclaw_agent::core::channels::telegram::{Poller, TelegramChannel};
#[cfg(feature = "esp32")]
use zenclaw_agent::esp32::http_client::EspHttpClient;
#[cfg(feature = "esp32")]
use zenclaw_agent::platform::http_client::HttpClient;
```

(If `Arc` is already imported in scope without a cfg gate, skip the first line.)

- [ ] **Step 2: Construct `Arc<EspHttpClient>` and Telegram resources after gateway construction**

Locate the section around `agent/src/main.rs:200-227` (the `let gateway = ...; ...; chat_tx ...; agent thread spawn` block). Just before the `// --- Start agent thread ...` comment, insert:

```rust
    // --- Construct shared HTTP client + optional Telegram resources ---
    let http: Arc<dyn HttpClient> = Arc::new(EspHttpClient::new());

    let tg_resources = config_for_tg
        .channels
        .telegram
        .as_ref()
        .filter(|t| t.enabled && !t.bot_token.is_empty())
        .map(|t| {
            (
                Poller::new(t.bot_token.clone()),
                TelegramChannel::new(t.bot_token.clone(), http.clone()),
                t.allowed_chat_ids.clone(),
            )
        });
```

- [ ] **Step 3: Update `agent_thread` invocation**

Replace the existing spawn block:

```rust
    {
        let gw = gateway.clone();
        let bot_token = config_for_tg.channels.telegram.as_ref()
            .filter(|t| t.enabled && !t.bot_token.is_empty())
            .map(|t| t.bot_token.clone());
        std::thread::Builder::new()
            .name("agent".into())
            .stack_size(32768)
            .spawn(move || agent_thread(bot_token.as_deref(), chat_rx, gw))
            .expect("Failed to spawn agent thread");
        log::info!("Agent thread started");
    }
```

with:

```rust
    {
        let gw = gateway.clone();
        let http_for_thread = http.clone();
        std::thread::Builder::new()
            .name("agent".into())
            .stack_size(32768)
            .spawn(move || agent_thread(chat_rx, gw, http_for_thread, tg_resources))
            .expect("Failed to spawn agent thread");
        log::info!("Agent thread started");
    }
```

- [ ] **Step 4: Replace `agent_thread` body**

Find `agent_thread` (`agent/src/main.rs:1342`) and replace it (and its leading doc comment) entirely. Replace from `/// Unified agent thread — handles both Telegram polling and HTTP chat requests.` through the closing `}` of the function with:

```rust
/// Unified agent thread — handles both Telegram polling and HTTP chat requests.
/// Single 32KB stack thread avoids the OOM from spawning a third thread.
#[cfg(feature = "esp32")]
fn agent_thread(
    chat_rx: std::sync::mpsc::Receiver<ChatRequest>,
    gateway: std::sync::Arc<zenclaw_agent::core::gateway::Gateway>,
    http: std::sync::Arc<dyn zenclaw_agent::platform::http_client::HttpClient>,
    mut tg: Option<(
        zenclaw_agent::core::channels::telegram::Poller,
        zenclaw_agent::core::channels::telegram::TelegramChannel,
        Option<Vec<String>>,
    )>,
) {
    if tg.is_some() {
        log::info!("Agent thread: Telegram + HTTP chat");
    } else {
        log::info!("Agent thread: HTTP chat only");
    }

    loop {
        // --- Process any pending HTTP chat requests (non-blocking) ---
        while let Ok(req) = chat_rx.try_recv() {
            log::info!(
                "HTTP chat: chat_id={} msg_len={}",
                req.chat_id,
                req.message.len()
            );
            zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Thinking);
            let result = esp_idf_svc::hal::task::block_on(
                gateway.chat(&req.chat_id, &req.message, "api"),
            );
            zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);
            let _ = req.reply_tx.send(result.map_err(|e| e.to_string()));
        }

        // --- Telegram poll (if enabled) ---
        if let Some((poller, channel, allowed)) = tg.as_mut() {
            let messages = match esp_idf_svc::hal::task::block_on(poller.poll_once(&*http, 10)) {
                Ok(m) => m,
                Err(e) => {
                    log::error!("Telegram poll: {}", e);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };

            for msg in messages {
                if let Some(ids) = allowed.as_ref() {
                    if !ids.contains(&msg.chat_id) {
                        log::warn!(
                            "Telegram message from disallowed chat: {}",
                            msg.chat_id
                        );
                        continue;
                    }
                }

                log::info!(
                    "Telegram msg from {}: {}B",
                    msg.chat_id,
                    msg.text.len()
                );

                if let Err(e) = esp_idf_svc::hal::task::block_on(
                    channel.send_typing(&msg.chat_id),
                ) {
                    log::warn!("Telegram send_typing: {}", e);
                }

                zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Thinking);
                let reply = match esp_idf_svc::hal::task::block_on(
                    gateway.chat(&msg.chat_id, &msg.text, "telegram"),
                ) {
                    Ok(r) => r,
                    Err(e) => format!("Error: {}", e),
                };
                zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);

                if let Err(e) = esp_idf_svc::hal::task::block_on(
                    channel.deliver(&msg.chat_id, &reply),
                ) {
                    log::error!("Telegram deliver: {}", e);
                } else {
                    log::info!("Telegram reply sent to {}", msg.chat_id);
                }
            }
        } else {
            // No Telegram — block-wait for HTTP chat requests so we don't busy-loop.
            if let Ok(req) = chat_rx.recv_timeout(std::time::Duration::from_secs(1)) {
                log::info!(
                    "HTTP chat: chat_id={} msg_len={}",
                    req.chat_id,
                    req.message.len()
                );
                zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Thinking);
                let result = esp_idf_svc::hal::task::block_on(
                    gateway.chat(&req.chat_id, &req.message, "api"),
                );
                zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);
                let _ = req.reply_tx.send(result.map_err(|e| e.to_string()));
            }
        }
    }
}
```

- [ ] **Step 5: Delete `tg_api`, `tg_http_get`, `tg_http_post`**

In `agent/src/main.rs`, delete the following functions and their `#[cfg(feature = "esp32")]` attributes (lines 1260-1329 in the pre-change file):

- The entire `// Telegram poller (ESP32 — blocking HTTP via esp-idf-svc)` comment block.
- `fn tg_api(token: &str, method: &str) -> String { ... }`
- `fn tg_http_get(url: &str) -> Result<String, String> { ... }`
- `fn tg_http_post(url: &str, json_body: &str) -> Result<String, String> { ... }`

Keep the `struct ChatRequest { ... }` definition that follows — it's still used.

---

## Task 13: ESP32 build verification

**Files:** none changed.

- [ ] **Step 1: Clean rebuild (esp-idf-sys cache is paranoid)**

```bash
cd agent
just clean devkitc
just build devkitc 2>&1 | tail -25
```
Expected: success. Build will take 5–10 minutes from cold cache.

If a compile error appears, the most likely causes are:
- An unused-import warning escalated to error: prune from `main.rs`.
- `Arc` ambiguity if it was imported under a different cfg gate: explicitly use `std::sync::Arc` in the imports we added.
- A signature mismatch on `agent_thread`: the new caller must match the new declaration exactly.

- [ ] **Step 2: Verify desktop still builds**

```bash
cd agent
cargo build --no-default-features --features desktop 2>&1 | tail -10
```
Expected: success.

- [ ] **Step 3: Run all tests one more time**

```bash
cd agent
cargo test --no-default-features --features desktop --lib 2>&1 | tail -10
```
Expected: all tests pass.

---

## Task 14: ESP32 hardware verification — post-change baseline + Telegram round-trip

**Files:**
- Modify: `docs/superpowers/specs/2026-05-01-telegram-unification-design.md` (record post-change numbers).

- [ ] **Step 1: Flash**

```bash
cd agent
just flash devkitc /dev/ttyACM0
```
Expected: flash completes; board boots; serial logs show `mDNS: <hostname>.local`.

- [ ] **Step 2: Wait and capture initial heap**

Wait ~12 seconds for WiFi connect + HTTP server up. Then:

```bash
HOST=zenclaw-XXX.local
curl -sf "http://$HOST/api/status" | jq '.memory'
```
Record as **`post_boot`**.

- [ ] **Step 3: Real Telegram round-trip**

From a Telegram client, send `ping` to your bot. Wait for the reply. Verify the reply arrives.

If `allowed_chat_ids` is set in config and your chat isn't in it, the bot will silently drop your message — make sure to use an allowed chat or unset the filter for the test.

- [ ] **Step 4: Capture post-roundtrip heap**

```bash
curl -sf "http://$HOST/api/status" | jq '.memory'
```
Record as **`post_after_roundtrip`**.

- [ ] **Step 5: Compute deltas, check acceptance bands**

| Metric | Pre | Post | Δ | Allowed Δ | OK? |
|---|---|---|---|---|---|
| free_kb at boot | (Task 1) | post_boot.free_kb | … | ±10KB | ? |
| used_kb at boot | (Task 1) | post_boot.used_kb | … | ±10KB | ? |
| free_kb after 1 RT | (Task 1) | post_after_roundtrip.free_kb | … | ±20KB | ? |
| used_kb after 1 RT | (Task 1) | post_after_roundtrip.used_kb | … | ±20KB | ? |
| Round-trip succeeds | yes | (this run) | — | — | hard gate |

If any cell is "not OK," do not commit yet — investigate. Check serial logs for panics or repeated TLS errors. Compare LLM reply sizes between runs (identical prompts should give comparable reply lengths; if not, that explains heap drift).

- [ ] **Step 6: Record post-change numbers in spec**

Edit `docs/superpowers/specs/2026-05-01-telegram-unification-design.md`. Below the pre-change baseline subsection, add a "Post-change verification" subsection with the same table filled in for `post_*` numbers, plus the deltas and a "all bands met / hard gate passed" sentence.

---

## Task 15: Commit B (feat: unify Telegram path)

**Files:** none new. Creates the second commit.

- [ ] **Step 1: Stage and inspect**

```bash
git add -A
git status
```
Expected staged:
- modified: `agent/src/core/channels/telegram.rs` (was an empty shell, now the full implementation + tests)
- modified: `agent/src/desktop/mod.rs`
- modified: `agent/src/desktop/run.rs`
- modified: `agent/src/main.rs`
- deleted: `agent/src/desktop/telegram.rs`
- modified: `docs/superpowers/specs/2026-05-01-telegram-unification-design.md`

- [ ] **Step 2: Diffstat sanity check**

```bash
git diff --cached --stat | tail -20
```
Rough expectation: `desktop/telegram.rs` deleted (~165 lines), `main.rs` shrinks (~190 lines net delete after substitution), `core/channels/telegram.rs` added (~360 lines including tests), `desktop/run.rs` modified. Net: a few hundred lines smaller across the diff.

- [ ] **Step 3: Commit**

```bash
git commit -m "$(cat <<'EOF'
feat: unify Telegram path through Channel + HttpClient traits

Second commit of the Telegram path unification slice. Replaces the two
parallel Telegram bot implementations (ESP32's hand-rolled main.rs
tg_http_get/post + JSON, and desktop's reqwest-based desktop/telegram.rs
+ inline spawn_telegram_loop sender) with a single canonical
implementation in core/channels/telegram.rs.

- core/channels/telegram.rs: Poller (long-poll receiver, exposes
  poll_once for caller-driven cadence; no poll_loop in core to keep
  tokio out of platform-neutral code), IncomingMessage,
  TelegramChannel (impl Channel; sends sendMessage / sendChatAction
  via &dyn HttpClient). 13 unit tests covering poll/parse and
  send/parse_mode behavior, all running against a MockHttpClient
  that needs no network or hardware.

- ESP32 main.rs: construct Arc<EspHttpClient> after NIC bring-up,
  hand it plus an optional (Poller, TelegramChannel, allowed_chat_ids)
  tuple to agent_thread. The interleaved single-thread loop preserved;
  Telegram block reduces to block_on(poller.poll_once(...)),
  block_on(channel.send_typing(...)), block_on(channel.deliver(...)).
  tg_api, tg_http_get, tg_http_post deleted (~75 lines).

- Desktop run.rs: construct Arc<ReqwestHttpClient>, pass to
  spawn_telegram_loop. Producer task calls poll_once in a loop,
  consumer constructs a TelegramChannel per spawned message-handling
  task. desktop/telegram.rs deleted (165 lines).

- ESP32 newly honors config.channels.telegram.allowed_chat_ids
  (previously ignored).

- Default-knob changes (documented in spec):
  - Long-poll timeout: 10s on both platforms (was 5s on ESP32, 10s
    on desktop). Halves Telegram API request rate.
  - parse_mode: None by default (was "Markdown" on ESP32). LLM
    replies aren't sanitized for Markdown special chars; a stray
    underscore returned 400. Recoverable via with_parse_mode.

- DevKitC verification: heap baseline pre/post within ±NN KB
  acceptance bands (recorded in spec); real Telegram round-trip
  confirmed working on hardware.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

(Replace `±NN KB` in the commit message with the actual measured deltas before running.)

- [ ] **Step 4: Confirm commit and clean tree**

```bash
git log --oneline -4
git status
```
Expected: top of log shows the new feat commit; status shows clean tree (modulo the unrelated WIP that was there at session start).

---

## Self-Review

Run after writing the plan, before handing off:

- **Spec coverage:** Each spec section maps to tasks?
  - Problem / goal — Tasks 2–10 fulfill.
  - Non-goals — none of the non-goals (subagents, background, gateway-channel wiring, streaming) appear in any task. ✓
  - Approach commits A & B — Tasks 4 and 15. ✓
  - Default-knob choices — applied in Tasks 7, 9 (parse_mode None default; 10s timeout; allowed_chat_ids in Tasks 10 and 12). ✓
  - Module layout — Tasks 2, 3, 5–10, 12. ✓
  - Public APIs — Tasks 5–10. ✓
  - Data flow — Tasks 10 (desktop), 12 (ESP32). ✓
  - Error handling — Task 7 (poll error path), Task 9 (deliver error), Task 12 (log + continue, never panic). ✓
  - Verification protocol — Tasks 1 (pre-baseline), 9 (unit tests), 11 (desktop manual), 13–14 (ESP32 hw). ✓
  - Risks — addressed implicitly by sticking to the design.

- **Placeholders:** scanned. None.

- **Type consistency:** `IncomingMessage`, `Poller`, `TelegramChannel`, `EspHttpClient`, `Channel` trait, `HttpClient` trait, `MockHttpClient` — all consistent across tasks.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-01-telegram-unification.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using `executing-plans`, batch execution with checkpoints.

**Which approach?**
