# ZenClaw Rust Agent — Design Spec

**Date:** 2026-04-11
**Status:** Approved
**Scope:** Full port of the MicroPython agent to Rust, desktop-first with ESP32-S3 (no PSRAM) as a hard constraint.

## Motivation

The MicroPython agent hits fundamental limits:

- **RAM**: On ESP32-S3 without PSRAM (512KB SRAM), MicroPython's interpreter + bytecode consumes ~400KB, forcing lazy loading, feature gating, and gc.collect() choreography. Even with these workarounds, 5 tool modules and cloud sync must be disabled.
- **IDF access**: MicroPython doesn't expose USB Host, leaving an 8GB USB stick inaccessible. Any IDF feature not wrapped by MicroPython is unreachable.
- **Build workflow**: Despite MicroPython's hot-reload promise, ZenClaw always builds a full LittleFS image and flashes. There is no iteration speed advantage over a compiled language.

A Rust agent executing from flash uses ~60KB of runtime RAM (vs ~400KB), has direct access to every IDF component, and follows the same build-and-flash workflow.

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Scope | Full parity port | All features: tools, sessions, Telegram, API, cron, heartbeat, subagents, MCP, memory, Google Sheets, cloud storage |
| Architecture | Compose lightweight crates (Approach B) | Own the agent loop, use libraries for I/O. No framework lock-in. |
| Phasing | Desktop-first, ESP32 later | Fast iteration on Linux, bring up ESP32 once core is solid |
| ESP32 target | Must run on no-PSRAM boards (512KB SRAM) | Hard constraint. Drives all platform abstraction decisions. |
| LLM providers | Two wire formats: OpenAI-compatible + Gemini | Covers 15+ providers. base_url determines format. |
| Skills | Dropped | Dynamic Python execution incompatible with Rust. Other tools cover the use cases. |
| Exec tool | Dropped | Same reason. LLM works through defined tools only. |
| Steering | Dropped | Cancel + re-send is simpler and covers the same use case. |
| Poller pause/resume | Dropped | Per-chat lock + cancellation prevents concurrent turns on the same session. |
| Data format | 100% compatible with MicroPython | Same config.json, same JSONL sessions, same memory files, same cron/jobs.json. Either agent can read the other's data. |

## Project Structure

```
zenclaw/
  firmware/              # existing MicroPython (untouched)
  web/                   # existing Nuxt UI (untouched)
  agent/                 # new Rust agent
    Cargo.toml
    src/
      main.rs
      config.rs

      core/              # platform-agnostic agent logic
        gateway.rs
        agent_loop.rs
        runner.rs
        prompt.rs
        tools/
          mod.rs
          file_tools.rs
          memory_tools.rs
          cron_tools.rs
          web_tools.rs
          message_tool.rs
          session_tools.rs
          gateway_tool.rs
          storage_tools.rs
          gsheets_tools.rs
          mcp_tools.rs
          subagent_tools.rs
        sessions/
          mod.rs
          jsonl.rs
          tree.rs
          compaction.rs
          state.rs
        memory/
          mod.rs
          embeddings.rs
          index.rs
          chunking.rs
        channels/
          mod.rs
          cli.rs
          telegram.rs
        background/
          mod.rs
          heartbeat.rs
          cron.rs
          subagents.rs

      platform/           # trait definitions
        mod.rs
        http_client.rs
        http_server.rs
        telegram.rs
        runtime.rs

      desktop/            # tokio ecosystem
        mod.rs
        runtime.rs        # tokio::spawn, tokio::time::sleep
        http_client.rs    # reqwest
        http_server.rs    # axum (SSE, WebSocket, CORS)
        telegram.rs       # teloxide

      esp32/              # esp-idf native (no tokio)
        mod.rs
        runtime.rs        # FreeRTOS tasks, esp-idf timers
        http_client.rs    # esp-idf-svc::http::client
        http_server.rs    # esp-idf-svc::http::server
        telegram.rs       # raw Bot API HTTP calls
        usb_host.rs       # USB MSC via esp-idf-sys FFI
```

## Build System

Cargo workspace with feature flags:

```toml
[features]
default = ["desktop"]
desktop = ["tokio", "axum", "reqwest", "teloxide", "genai", "usearch"]
esp32 = ["esp-idf-svc", "esp-idf-sys"]
hnsw = ["usearch"]  # optional, desktop or PSRAM boards only
```

- `cargo run` — builds and runs desktop agent
- `cargo build --no-default-features --features esp32 --target xtensa-esp32s3-espidf` — builds for ESP32-S3

## Platform Abstraction

The core agent depends on traits, never on platform crates directly.

### HttpClient

```rust
#[async_trait]
trait HttpClient: Send + Sync {
    async fn get(&self, url: &str, headers: &Headers) -> Result<Response>;
    async fn post(&self, url: &str, headers: &Headers, body: &[u8]) -> Result<Response>;
    async fn stream(
        &self, url: &str, headers: &Headers, body: &[u8],
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<()>;
}
```

- Desktop: `reqwest` with `eventsource-stream` for SSE parsing.
- ESP32: `esp-idf-svc::http::client` with connection reuse. Global semaphore limits concurrent TLS connections to 2.

### HttpServer

```rust
#[async_trait]
trait HttpServer: Send + Sync {
    async fn start(&self, port: u16, state: AppState) -> Result<()>;
}
```

- Desktop: `axum` with built-in SSE, WebSocket, CORS via `tower-http`.
- ESP32: `esp-idf-svc::http::server` (~10KB). Manual route registration, no middleware.

### Runtime

```rust
#[async_trait]
trait Runtime: Send + Sync {
    fn spawn<F: Future<Output = ()> + Send + 'static>(&self, f: F);
    async fn sleep(&self, ms: u64);
}
```

- Desktop: `tokio::spawn`, `tokio::time::sleep`.
- ESP32: FreeRTOS tasks, `esp-idf-svc` timers.

### LlmProvider

```rust
#[async_trait]
trait LlmProvider: Send + Sync {
    async fn chat(
        &self, messages: &[Message], tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<LlmResponse>;

    async fn chat_stream(
        &self, messages: &[Message], tools: &[ToolDefinition],
        config: &ModelConfig, on_delta: &mut dyn FnMut(&str),
    ) -> Result<LlmResponse>;
}
```

- Desktop: `GenaiProvider` wraps the `genai` crate, implementing `LlmProvider` trait. Native OpenAI + Gemini wire formats.
- ESP32: `EspProvider` implements `LlmProvider` using the platform `HttpClient` directly. OpenAI format is trivial. Gemini format conversion (~200 lines) ported from MicroPython `providers/__init__.py`.
- Provider routing: config maps provider names to `{api_key, base_url, model}`. The `base_url` determines wire format: contains `googleapis.com` = Gemini, everything else = OpenAI-compatible.

## Platform Comparison

| Component | Desktop | ESP32 (no PSRAM) | ESP32 RAM cost |
|---|---|---|---|
| Async runtime | tokio | FreeRTOS (already in IDF) | 0 |
| HTTP server | axum | esp-idf-svc httpd | ~10KB |
| HTTP client | reqwest | esp-idf-svc http client | ~8KB |
| Telegram | teloxide | raw Bot API over HttpClient | 0 extra |
| LLM providers | genai | direct HTTP + hand-built JSON | 0 extra |
| TLS | rustls | mbedtls (HW accelerated, in IDF) | ~40KB/conn |
| Vector search | usearch (HNSW) | brute-force cosine | 0 index overhead |
| USB host MSC | n/a | esp-idf-sys FFI | ~5KB |
| **Total runtime** | **~400KB** | **~60KB** | |

ESP32 connection budget: max 2 concurrent TLS connections (one inbound API, one outbound LLM/Telegram). Keep-alive reuses sessions for repeated calls to the same host.

## Core Agent Loop

```
gateway::chat(message, chat_id, channel)
  -> session.append(user_message)
  -> prompt::build_system_prompt(config, tools, context)
  -> agent_loop::run_loop()
       |-- runner::call_llm(provider, messages, tools)
       |-- parse response: text / tool_calls / mixed
       |-- if tool_calls:
       |     |-- tool_registry.execute(call) for each
       |     |-- session.append(tool_results)
       |     |-- loop_detector.check() -> break if stuck
       |     |-- check cancellation
       |     +-- continue loop
       |-- if text:
       |     |-- channel.deliver(text)
       |     +-- session.append(assistant_message)
       +-- repeat until done or cancelled
```

### Cancellation

`tokio_util::sync::CancellationToken` (desktop) or a simple `AtomicBool` (ESP32). Triggered by `/api/chat/cancel` or Telegram `/stop`. Checked after each tool execution.

### Concurrent Chat Prevention

Per-chat lock via `HashMap<String, CancellationToken>`. New message on a busy chat_id cancels the running turn, waits for it to stop, then starts fresh. No steering, no pause/resume.

### Loop Detection

Port of MicroPython `tool_loop.py`. Count repeated tool call patterns. Break after threshold (default: 3 identical patterns). Circuit breaker for stuck loops.

### Retry

Exponential backoff, max 3 attempts. Retryable: 429 (rate limit), 5xx (server error). Not retryable: 401/403 (auth), 400 (bad request).

## Tool System

Action-param pattern preserved. Each tool module registers one tool with an `action` field.

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult>;
}

struct ToolContext {
    chat_id: String,
    config: Arc<Config>,
    session_manager: Arc<SessionManager>,
    memory_store: Arc<MemoryStore>,
}

enum ToolResult {
    Text(String),
    FileData { name: String, mime: String, data: Vec<u8> },
}
```

Static registration at startup. Conditional registration based on config (storage requires S3 keys, gsheets requires google config, etc.). No lazy loading needed — Rust code lives in flash, not RAM.

### Tool Inventory

**Always registered:**
- `file` (read, write, edit, delete, list_dir)
- `memory` (save, search, get, reindex)
- `cron` (add, list, remove, run, update)
- `web` (web_fetch, web_search, hub_search, hub_install)
- `message_send` (cross-channel delivery)
- `session` (status, list, history, reset, branch)
- `gateway` (status, reload)

**Conditionally registered:**
- `storage` (read, write, delete, list, info, read_chunk, grep, analyze) — requires S3 config
- `gsheets` (read, write, append, clear) — requires Google config
- `mcp` (connect, list_tools, call, disconnect, servers) — requires MCP config, uses `rmcp` crate
- `subagents` (spawn, list, cancel) — always available on desktop, sequential-only on ESP32

## Session System

JSONL branching tree, identical format to MicroPython. Full read/write compatibility.

### Entry Types

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum SessionEntry {
    #[serde(rename = "message")]
    Message {
        id: String,
        parent: Option<String>,
        role: Role,
        content: MessageContent,
        tool_calls: Option<Vec<ToolCall>>,
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
```

### Storage

- Session files: `data/sessions/{chat_id}.jsonl`
- Session metadata: `data/sessions.json`
- Append-only writes (append to file, update in-memory tree)
- Compaction rewrites the file when token threshold exceeded

### Volatile State

Per-chat `SessionState` (turn count, model override, active channel) in `HashMap<String, SessionState>`, persisted to `data/sessions.json`.

## Telegram Integration

Use `teloxide` (desktop) for Bot API types and HTTP. Own the polling loop.

### Polling

Continuous long-poll. No pause/resume. Per-chat lock prevents concurrent turns.

```
loop:
  updates = bot.get_updates(offset, timeout=10)
  for update in updates:
    offset = update.id + 1
    msg = extract_message(update)
    bot.send_chat_action(msg.chat_id, Typing)
    gateway.chat(msg.text, msg.chat_id, Channel::Telegram(msg.chat_id))
```

### Streaming

- **DMs**: No streaming. `send_message()` with final text.
- **Groups**: `TelegramStreamWriter` accumulates deltas, calls `edit_message_text()` on ~1s debounce timer.

### Media

- Photos: download via `get_file()` + HTTP fetch, convert to base64 image message.
- Voice/audio: download, pass to transcription provider.
- Documents: promote image documents to photo messages.

### Forum Topics

Route by `{chat_id}:{thread_id}` as session key. `teloxide` exposes `message_thread_id`.

### ESP32 Implementation

No `teloxide`. Raw HTTP calls via platform `HttpClient`:
- `GET https://api.telegram.org/bot{token}/getUpdates?offset={n}&timeout=10`
- `POST https://api.telegram.org/bot{token}/sendMessage`
- `POST https://api.telegram.org/bot{token}/editMessageText`
- Parse JSON responses with `serde_json`.

## API Server

Matches current Microdot endpoints. Nuxt web UI works unchanged.

### Routes

| Method | Path | Handler |
|---|---|---|
| GET | `/api/status` | Device info, uptime, memory, WiFi |
| POST | `/api/chat` | Send message, SSE streaming response |
| POST | `/api/chat/cancel` | Cancel active chat |
| GET | `/api/config` | Read config.json |
| PUT | `/api/config` | Update config.json |
| GET | `/api/files/*path` | Read workspace file |
| PUT | `/api/files/*path` | Write workspace file |
| DELETE | `/api/files/*path` | Delete workspace file |
| WS | `/ws` | Bidirectional chat + streaming |

### Streaming Chat

`/api/chat` returns SSE stream. Agent loop sends deltas through a channel, handler converts to SSE events. On ESP32, the native httpd supports chunked transfer encoding for streaming.

### Desktop

`axum` with `CorsLayer::permissive()`. TLS optional via `axum-server` + rustls.

### ESP32

`esp-idf-svc::http::server`. Manual route registration. TLS via IDF's mbedtls. Self-signed cert from `data/certs/`.

## Memory System

### Storage Format (compatible with MicroPython)

- `data/MEMORY.md` — persistent memory, capped at 32KB
- `data/memory/YYYY-MM-DD.md` — daily memory files, max 30
- `data/memory/index.json` — vector index (chunks + embeddings)

### Vector Search

**Brute-force mode** (default, all platforms):
- Cosine similarity against all stored embeddings
- For <1000 chunks, instant even on ESP32
- Zero RAM overhead for index structures

**HNSW mode** (feature flag `hnsw`, desktop or PSRAM boards):
- `usearch` crate with disk persistence
- i8 quantization option for memory savings

### VectorStore Trait

```rust
#[async_trait]
trait VectorStore: Send + Sync {
    async fn add(&mut self, id: &str, text: &str, embedding: &[f32]);
    async fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<MemoryMatch>;
    fn save(&self) -> Result<()>;
    fn load(&mut self) -> Result<()>;
}
```

### Embeddings

POST to configured embedding API via platform `HttpClient`. Same provider config as LLM calls.

## Background Tasks

Single background loop. One async task on desktop (tokio::spawn), one FreeRTOS-friendly loop on ESP32.

```
background_loop:
  every 1s:
    check cron jobs -> run if due
    check heartbeat timer -> run if due
    reap finished subagents
```

### Cron

- `cron` crate for expression parsing (desktop), `croner` for no_std (ESP32)
- Job state persisted to `data/cron/jobs.json`
- Same format as MicroPython

### Heartbeat

- Periodic autonomous chat at configurable interval
- Reads `data/HEARTBEAT.md` for checklist
- Runs in background loop, not a separate task

### Subagents

- Desktop: `tokio::spawn` for concurrent subagents
- ESP32: sequential execution in background loop (one at a time)
- Max spawn depth enforced (default: 3)
- Registry tracks active subagents per parent session

## MCP Integration

`rmcp` crate (official Rust MCP SDK, 3,283 stars, v1.4). Feature-gated — not compiled for minimal ESP32 builds.

Supports: stdio, SSE, and WebSocket transport. Tool discovery, tool calling, resource access. Bridges MCP tools into the ZenClaw tool registry.

## Config System

Same `config.json` format as MicroPython. Serde structs with `#[serde(default)]` for defaults.

```rust
#[derive(Deserialize)]
struct Config {
    providers: ProvidersConfig,
    agent_name: Option<String>,
    channels: ChannelsConfig,
    heartbeat: HeartbeatConfig,
    memory: MemoryConfig,
    compaction: CompactionConfig,
    search: SearchConfig,
    storage: Option<StorageConfig>,
    google: Option<GoogleConfig>,
    mcp: Option<McpConfig>,
    hub_url: Option<String>,
}
```

Desktop: `config-rs` for layered loading (file + env overrides).
ESP32: `serde_json::from_str()` on config.json contents. NVS for WiFi credentials (same as current).

## Crate Dependencies

### Desktop (`--features desktop`)

| Crate | Purpose |
|---|---|
| `tokio` | async runtime |
| `axum` + `tower-http` | HTTP server, SSE, WebSocket, CORS |
| `reqwest` + `eventsource-stream` | HTTP client, SSE parsing |
| `teloxide` | Telegram Bot API |
| `genai` | multi-provider LLM calls (OpenAI + Gemini) |
| `rmcp` | MCP client |
| `serde` + `serde_json` | serialization, JSONL |
| `config` | layered config loading |
| `cron` | cron expression parsing |
| `rust-s3` | S3-compatible cloud storage |
| `google-sheets4` + `yup-oauth2` | Google Sheets API |
| `usearch` | HNSW vector search (optional) |
| `tokio-util` | CancellationToken |
| `async-trait` | async trait support |
| `tracing` | structured logging |

### ESP32 (`--features esp32`)

| Crate | Purpose |
|---|---|
| `esp-idf-svc` | WiFi, NVS, HTTP server/client, GPIO |
| `esp-idf-sys` | raw IDF FFI (USB Host MSC) |
| `esp-idf-hal` | hardware abstraction |
| `serde` + `serde_json` | serialization, JSONL |
| `croner` | cron parsing (no_std compatible) |
| `async-trait` | async trait support |

## USB Host MSC (ESP32 only)

Thin unsafe FFI wrapper around ESP-IDF's `usb_host_msc` component (~100 lines):

1. `usb_host_install()` — initialize USB host library
2. `msc_host_install()` — register MSC class driver
3. `msc_host_vfs_register()` — mount FAT filesystem at `/usb`
4. Standard `std::fs` operations on `/usb/*`

FAT-formatted drives only. Auto-detect on boot, mount if present.

## What Was Dropped

| Feature | Reason |
|---|---|
| Dynamic skills (Python exec from `data/skills/`) | Incompatible with Rust's compiled nature |
| Exec tool (arbitrary code execution) | Same reason. LLM works through defined tools only. |
| Steering (inject messages mid-turn) | Cancel + re-send is simpler, covers the same use case |
| Poller pause/resume | Per-chat lock + cancellation prevents concurrent turns |

## What Was Gained

| Feature | How |
|---|---|
| USB Host MSC (8GB flash drive) | esp-idf-sys FFI, ~100 lines |
| No-PSRAM ESP32 support (all features) | ~60KB runtime vs ~400KB MicroPython |
| Type safety | Compile-time checks, no runtime `KeyError` |
| Single binary deployment | One file to flash, no LittleFS image |
| Hardware TLS acceleration | mbedtls in ESP-IDF uses ESP32's AES/SHA hardware |
