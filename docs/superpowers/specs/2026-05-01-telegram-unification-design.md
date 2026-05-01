# Telegram path unification — design

**Date:** 2026-05-01
**Branch:** `main` (work to be done in a feature branch off `main`)
**Scope:** Slice (a) of the T-Dongle-era amputation revival arc. Other slices (Channel→Gateway wiring, subagents revival, background runner) are out of scope and deferred to separate sessions.

## Problem

`agent/src/main.rs` and `agent/src/desktop/{run.rs,telegram.rs}` contain two parallel implementations of the Telegram bot protocol:

- **ESP32** (`main.rs:1260-1450`): hand-rolled `tg_http_get` / `tg_http_post` using `EspHttpConnection`, hand-built JSON for `sendMessage` / `sendChatAction`, manual escaping.
- **Desktop** (`desktop/telegram.rs` + `desktop/run.rs:111-176`): `TelegramPoller` with `reqwest::Client`, plus inline `reqwest::Client.post(...)` calls in `spawn_telegram_loop`.

A `Channel` trait exists in `desktop/channels/mod.rs` with a `TelegramChannel` impl, but **nothing in the codebase calls it** — it's dead code. Same for `CliChannel` and `ChannelKind`. An `HttpClient` trait exists in `platform/http_client.rs` with a `ReqwestHttpClient` impl in `desktop/`, but ESP32 has no impl and bypasses the trait entirely.

The reason for the duplication is historical: the no-PSRAM T-Dongle-S3 era forced ESP32 onto a minimal hand-rolled HTTP path with a single `TLS_MUTEX`. That constraint is gone (DevKitC has 8MB PSRAM, the floor as of 2026-04-30). The two-implementation shape was never reconciled.

## Goal

One canonical Telegram implementation in `core/channels/telegram.rs`, sitting on top of the existing `Channel` and `HttpClient` traits, used identically by ESP32 and desktop. ESP32 keeps its single-threaded `agent_thread` interleaved loop; desktop keeps its tokio task structure. Both go through the same parsing, JSON-building, and HTTP code.

## Non-goals

- **Channel→Gateway wiring** (so `gateway.chat()` accepts a `&dyn Channel` and delivers internally). Deferred to a separate session.
- **Subagents revival.** Deferred.
- **Background runner unification.** Deferred.
- **Streaming via `editMessageText`.** The `Channel::deliver_stream` method exists with a default impl that calls `deliver`; real streaming can override it later without touching the trait.
- **LLM runner migration to `HttpClient` trait.** `esp32/runner.rs` keeps its own `EspHttpConnection` for now; can migrate later in a follow-up.

## Approach

Two commits:

- **Commit A — `refactor: hoist Channel trait + add EspHttpClient`** (runtime semantics unchanged; new code is unreachable from existing call sites)
  - Move `Channel` trait from `desktop/channels/mod.rs` to `core/channels/mod.rs`. Drop the `#[cfg(feature = "desktop")]` gate on the trait itself.
  - Drop `ChannelKind` enum and the `kind()` method (over-design — single-variant after CliChannel removal).
  - Delete `CliChannel` and the `desktop/channels/` module entirely (dead code; confirmed zero callers).
  - Add `core/channels/mod.rs` exposing `Channel` and `deliver_stream` with a default impl that calls `deliver`.
  - Add `agent/src/esp32/http_client.rs` with `EspHttpClient` implementing `HttpClient`. Internally takes the existing `lib.rs::TLS_MUTEX` per-call. Stateless apart from a configurable timeout.
  - No callers of `EspHttpClient` yet — the struct compiles and is exported but unused.
  - **Verification:** `cargo build --no-default-features --features desktop` and `just build devkitc` both succeed. No on-device run required (no behavior change).

- **Commit B — `feat: unify Telegram path through Channel + HttpClient traits`** (the feature)
  - Add `core/channels/telegram.rs` with:
    - `pub struct Poller { bot_token: String, offset: i64 }` exposing `new(token)` and `async fn poll_once(&mut self, http: &dyn HttpClient, timeout_secs: u32) -> Result<Vec<IncomingMessage>, ...>`. No `poll_loop` — keeps `tokio::sync::mpsc` out of `core/`.
    - `pub struct IncomingMessage { chat_id, text, from_username }`.
    - `pub struct TelegramChannel { bot_token, http: Arc<dyn HttpClient>, parse_mode: Option<String> }` with `new(token, http)`, `with_parse_mode(opt)`, and `async fn send_typing(&self, chat_id: &str)`. Implements `Channel`.
    - Default `parse_mode = None` (safer than ESP32's current `"Markdown"`, since LLM replies aren't sanitized for Markdown special chars).
  - Migrate `main.rs` (ESP32):
    - Construct `Arc<dyn HttpClient>` (= `Arc::new(EspHttpClient::new())`) after NIC bring-up.
    - Construct `Poller` and `TelegramChannel` if `config.channels.telegram.enabled`.
    - Replace the inline Telegram block in `agent_thread` (~120 lines) with calls to `poller.poll_once(&*http, 10)`, `channel.send_typing(...)`, `channel.deliver(...)`. Each call wrapped in `block_on` (same pattern as the existing `block_on(gateway.chat(...))`).
    - Newly honor `config.channels.telegram.allowed_chat_ids`.
    - Delete `tg_api`, `tg_http_get`, `tg_http_post` (~75 lines).
  - Migrate `desktop/run.rs`:
    - Construct `Arc<dyn HttpClient>` (= `Arc::new(ReqwestHttpClient::new())`).
    - Replace inline `reqwest::Client.post(...)` in `spawn_telegram_loop` with `Poller::poll_once` (in a tokio loop) and `TelegramChannel::deliver` / `send_typing`.
    - Delete `desktop/telegram.rs` (165 lines).
  - **Verification:** see "Verification protocol" below.

## Default-knob choices

Three behavioral defaults that diverge from one of the two existing implementations:

| Knob | Old (ESP32) | Old (desktop) | New (both) | Why |
|---|---|---|---|---|
| Long-poll timeout | 5s | 10s | 10s | Halves Telegram API request rate from ESP32; matches desktop. Configurable via the `timeout_secs` arg to `poll_once`. |
| `parse_mode` on `sendMessage` | `"Markdown"` | (omitted) | `None` (omitted) | LLM replies aren't sanitized for Markdown special chars (`_`, `*`, `[`, `` ` ``); a stray underscore returns 400. Recoverable via `TelegramChannel::with_parse_mode(Some("Markdown"))`. |
| `allowed_chat_ids` enforcement | (ignored) | enforced | enforced (both platforms) | Honors a config field that ESP32 was already accepting but silently ignoring. If the field is unset/empty, behavior is unchanged ("allow all"). |

## Module layout (after Commit B)

```
agent/src/
  core/
    channels/
      mod.rs          ← Channel trait (moved from desktop/, no cfg gate)
      telegram.rs     ← NEW: Poller + TelegramChannel (impl Channel).
                          Both take &dyn HttpClient. No TLS knowledge.
  platform/
    http_client.rs    ← HttpClient trait (unchanged)
  esp32/
    http_client.rs    ← NEW: EspHttpClient impl HttpClient
    runner.rs         ← unchanged this slice
  desktop/
    http_client.rs    ← ReqwestHttpClient (unchanged)
    run.rs            ← Construct Arc<ReqwestHttpClient>, use new APIs
    telegram.rs       ← DELETED (subsumed by core/channels/telegram.rs)
  main.rs             ← Construct Arc<EspHttpClient> after NIC up;
                          agent_thread keeps interleaved single-thread shape
                          but calls Poller::poll_once + TelegramChannel.
                          tg_api/tg_http_get/tg_http_post DELETED.
```

## Public APIs

### `core/channels/mod.rs`

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    async fn deliver(&self, chat_id: &str, text: &str)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn deliver_stream(&self, chat_id: &str, chunk: &str)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        self.deliver(chat_id, chunk).await
    }
}
```

### `core/channels/telegram.rs`

```rust
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub chat_id: String,
    pub text: String,
    pub from_username: Option<String>,
}

pub struct Poller { /* … */ }

impl Poller {
    pub fn new(bot_token: String) -> Self;
    pub async fn poll_once(
        &mut self,
        http: &dyn HttpClient,
        timeout_secs: u32,
    ) -> Result<Vec<IncomingMessage>, Box<dyn std::error::Error + Send + Sync>>;
}

pub struct TelegramChannel { /* … */ }

impl TelegramChannel {
    pub fn new(bot_token: String, http: Arc<dyn HttpClient>) -> Self;
    pub fn with_parse_mode(self, mode: Option<String>) -> Self;
    pub async fn send_typing(&self, chat_id: &str)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

#[async_trait]
impl Channel for TelegramChannel { /* … */ }
```

### `esp32/http_client.rs`

```rust
pub struct EspHttpClient { timeout: Duration }

impl EspHttpClient {
    pub fn new() -> Self;
    pub fn with_timeout(self, t: Duration) -> Self;
}

#[async_trait]
impl HttpClient for EspHttpClient {
    async fn get(&self, url: &str, headers: &Headers) -> Result<Response, ...> {
        // 1. Take TLS_MUTEX (existing static in lib.rs), unwrap_or_else(into_inner)
        //    on poison — only forward path is to ignore poison and try again.
        // 2. Build EspHttpConnection with crt_bundle_attach + headers
        // 3. initiate_request → initiate_response → read body
        // 4. Drop connection (releases mutex)
    }
    async fn post(&self, url, headers, body) -> Result<...>;
    async fn put(&self, url, headers, body)  -> Result<...>;
    async fn delete(&self, url, headers)     -> Result<...>;
    async fn stream_post(&self, url, headers, body, on_chunk) -> Result<...>;
}
```

## Data flow (one inbound message → reply)

```
                Telegram API
                     │
              GET /getUpdates?offset=N&timeout=10
                     │  (held open up to 10s)
                     ▼
              poller.poll_once(&*http, 10)
                     │
                     │   http.get(url, &{})
                     ▼
            EspHttpClient or ReqwestHttpClient
                     │   ┌── ESP32: takes TLS_MUTEX, EspHttpConnection
                     │   └── Desktop: reqwest connection pool
                     ▼
              Response { status, body: Vec<u8> }
                     │
              Poller parses JSON, advances offset
                     │
                     ▼
              Vec<IncomingMessage> ────► caller
                     │
                     │   allowed_chat_ids filter (per-platform call site)
                     ▼
         ┌────────── for each accepted msg ──────────┐
         │                                            │
         ▼                                            ▼
  channel.send_typing(id)         gateway.chat(id, text, "telegram")
         │                                            │
   http.post(sendChatAction)            (LLM + tool loop → String)
         │                                            │
         └─────────────────┬──────────────────────────┘
                           ▼
                   channel.deliver(id, reply)
                           │
                   http.post(sendMessage)
                           │
                   non-200 → log; never panic
                           │
                           ▼
                      next iteration
```

## Error handling

Single load-bearing rule: **no panics inside the Telegram path.** Every fallible call is `Result`-handled; the worst outcome is "this one message lost, log it, keep polling."

| Failure point | User-visible | Action | Continues? |
|---|---|---|---|
| `getUpdates` network error | Bot silent ~5s | `log::error!`, sleep 5s | Yes |
| `getUpdates` JSON parse | Same | `log::error!`, sleep 5s | Yes |
| `getUpdates` 401 (bad token) | Bot silent | `log::error!` per attempt | Yes (retry forever — punt smart-shutdown) |
| `gateway.chat` returns `Err` | User receives `Error: {e}` reply | (already logged in gateway) | Yes |
| `sendChatAction` (typing) error | No typing indicator | `log::warn!` | Yes (typing is cosmetic) |
| `sendMessage` error | This reply lost | `log::error!` | Yes |
| Bot blocked by user (403) | Reply lost for that user | `log::error!` | Yes |
| `TLS_MUTEX` poisoned | None (recovered) | `unwrap_or_else(into_inner)`, no log | Yes |
| `mbedTLS` handshake fail after poison | Bot silent until network recovers | `log::error!` | Yes |

**Retry policy:** Poller sleeps 5s on `Err` and loops; no exponential backoff. Send (`deliver` / `send_typing`) has no retry — failed sends are dropped. Matches today's behavior.

**Cancellation:** handled at the `Gateway` layer via the existing `active_chats` per-chat cancel flag. Telegram path doesn't need to know about it.

**Logging:** `log::*` macros in `core/channels/telegram.rs` (matches existing `core/` style). `EspHttpClient` uses `log::*` too. No `tracing` dependency in core.

**`TLS_MUTEX`:** stays in `lib.rs` for this slice. `esp32/runner.rs` still references it directly; can move to `esp32/http_client.rs` once the runner also migrates (separate slice).

## Verification protocol

### Unit tests (in `core/channels/telegram.rs::tests`)

Run via `cargo test --features desktop`. A `MockHttpClient` (test-only, in the same file) implements `HttpClient` with canned responses + request recording. Coverage:

- `Poller::poll_once`:
  1. Empty `result` array → `Ok(vec![])`.
  2. One text message → returns one `IncomingMessage`; offset advances to `update_id + 1`.
  3. Multiple updates → all returned; offset advances to max + 1.
  4. Update with no `message` field (e.g. `callback_query`) → skipped.
  5. Update with non-text message (photo-only) → skipped.
  6. Malformed JSON body → returns `Err`.
  7. Non-200 HTTP status → returns `Err` with status in message.
- `TelegramChannel::deliver`:
  8. Body contains `chat_id` + `text`; URL contains token + `sendMessage`.
  9. With `with_parse_mode(Some("Markdown"))`: body includes `"parse_mode":"Markdown"`.
  10. Default (`None`): body has no `parse_mode` key.
  11. Non-200 response → returns `Err`.
- `TelegramChannel::send_typing`:
  12. Posts to `sendChatAction` with `"action":"typing"`.
- `Channel` trait default impl:
  13. `deliver_stream` routes through `deliver` with the same args.

### On-device verification (DevKitC, Commit B only)

Commit A skips on-device — `just build devkitc` succeeding suffices.

**Pre-change baseline** (run on `main` *before* starting Commit A; numbers recorded back into this doc):

```bash
just build devkitc && just flash devkitc /dev/ttyACM0
# wait ~12s for boot
HOST=zenclaw-XXX.local
curl -sf http://$HOST/api/status | jq '.memory'
# Send "ping" from a Telegram client to the bot; wait for reply
curl -sf http://$HOST/api/status | jq '.memory'
```

Capture: `internal_free`, `largest_free_block`, `psram_free` at boot and after one round-trip.

**Post-Commit-B** (after the feature lands; `just clean devkitc` first):

Same protocol, same fields captured.

**Acceptance gates:**

| Metric | Allowed Δ |
|---|---|
| Boot internal SRAM free | ±10KB |
| Internal SRAM free after 1 round-trip | ±20KB |
| Largest free block at idle | ±50KB |
| PSRAM free at idle | informational |
| Real Telegram round-trip succeeds | hard gate |

**Hard fails:** any panic, any boot loop, any TLS handshake error on a request type that succeeded during the pre-change baseline, or the Telegram round-trip not completing.

### Desktop sanity check

```bash
cargo build --no-default-features --features desktop
cargo run --features desktop          # config.json with bot_token
# Send a Telegram message; verify reply
```

## Code change estimate

- **Added:** ~250 lines (`core/channels/telegram.rs` ~180 + `esp32/http_client.rs` ~70).
- **Deleted:** ~360 lines (`desktop/telegram.rs` 165 + `main.rs` Telegram inline ~190 + dead `CliChannel` ~30).
- **Net:** ~−110 lines, with the surviving implementation single-sourced in `core/`.

## Risks

- **`async_trait` + `EspHttpConnection` (blocking) interaction.** ESP-IDF `EspHttpConnection` is synchronous; wrapping it inside an `async fn` body just runs it inline when the future is polled. This works (the existing `esp32/runner.rs` does the same), but readers who expect cooperative async should know it's "fake async" on ESP32. Documented in `EspHttpClient`'s doc comment.
- **`block_on` re-entrancy.** ESP32 calls `block_on(poller.poll_once(...))` from `agent_thread`. Since `poll_once` doesn't itself call `block_on`, no nesting. Safe.
- **TLS_MUTEX held during full HTTP request.** A long Telegram long-poll holds the mutex for up to 10s. Today's code does the same (held in `tg_http_get`). On ESP32, only `agent_thread` and the LLM runner take this mutex, and `agent_thread` is single-threaded — no contention with itself. The LLM runner runs on the same thread (`block_on` from `agent_thread` for HTTP-chat handling). No deadlock, no contention.
- **Default-knob changes** documented in their own section above (`parse_mode`, long-poll timeout, `allowed_chat_ids`). All three are intentional and recoverable.

## Open questions

None — all design questions resolved during brainstorming session 2026-05-01.
