# Streaming chat events — design

**Date:** 2026-05-01
**Branch:** `main` (work to be done in a feature branch off `main`)
**Scope:** Surface the agent's per-turn activity (tool calls, tool results, "thinking" between LLM calls) over `/ws/chat` as discrete events, and render them in the web chat UI as inline disclosures. End the "blackbox" feeling where users see only the final reply.

## Problem

`/ws/chat` is a WebSocket in name only — the ESP32 handler (`agent/src/main.rs:1178-1219`) and desktop handler (`agent/src/desktop/server.rs:492-532`) both `block_on(gateway.chat(...))` and emit a single `{type:"done", text:reply}` frame at the end. `/api/chat/history` (`main.rs:707-736`, `desktop/server.rs:176-213`) returns `{role, content}` only and explicitly filters out `content.is_empty()`, which silently drops every assistant turn that was tool-calls-only. The chat UI (`web/app/pages/chat.vue`) maps everything to `parts: [{type:'text', text}]` — there is no rendering branch for tool activity.

Net effect: a turn that runs three tools and then replies "Done." appears in the UI as just "Done." — and on page reload the tool history is gone too.

The data is *already* in the right shape on disk. `gateway.rs:112-121` persists `tool_calls` and `tool_call_id` to the session JSONL as part of the optimization arc. The bottleneck is purely the wire format between the agent and the browser.

## Goal

A typed-event stream over `/ws/chat` that fires at the natural seams in `agent_loop::run_loop`. The web chat UI renders each tool call as a compact inline disclosure ("🔧 memory_search ▸") that expands to show args + result. Page reload replays the same disclosures from `/api/chat/history`, which adopts the same event-shaped payload.

## Non-goals

- **Token-level streaming of assistant text.** Each `runner.call` returns one whole message; the `assistant_text` event carries it as a single chunk. Per-provider SSE plumbing (Gemini `streamGenerateContent`, OpenAI `data:` SSE, GLM variant) is deferred. The schema is forward-compatible — adding `assistant_text_delta` later doesn't change existing event types.
- **REST `/api/chat` shape change.** Headless callers (curl, scripts, Telegram-internal-via-gateway) keep the existing `{reply}` response. Only `/ws/chat` and `/api/chat/history` change shape; the web UI moves to WS.
- **Tool result fetching by ID.** Results are sent inline in the WS frame. The agent's existing `cap_or_refuse` (`agent_loop.rs:420`) caps results at 256KB on ESP32 / uncapped on desktop — comfortable for inline transport.
- **Visibility for circuit-breaker / compaction / retry events.** v1 surfaces `thinking`, `tool_call_*`, `assistant_text`, `done`, `error`. Other agent-loop machinery stays in the log ring.

## Approach

A single `events: Option<&Sender<ChatEvent>>` parameter threads down the existing call chain (`Gateway::chat` → `agent_loop::run_loop` → `execute_tool_calls`). REST callers pass `None` (no-op); WS handlers pass `Some`. No parallel `chat_streaming()` / `run_loop_streaming()` duplicates.

### Event protocol

**Inbound (browser → server) over `/ws/chat`:**
```json
{ "type": "user_message", "chat_id": "web", "text": "..." }
{ "type": "cancel", "chat_id": "web" }
```

**Outbound (server → browser):**
```json
{ "type": "user_message", "chat_id": "web", "text": "..." }   // from history replay only
{ "type": "thinking_started" }
{ "type": "thinking_ended" }
{ "type": "tool_call_started",  "id": "call_abc", "name": "memory_search", "args": { ... } }
{ "type": "tool_call_finished", "id": "call_abc", "ok": true,  "result": "..." }
{ "type": "tool_call_finished", "id": "call_abc", "ok": false, "error":  "..." }
{ "type": "assistant_text", "text": "...", "final": true }
{ "type": "done" }
{ "type": "error", "error": "..." }
```

Notes:
- `thinking_*` brackets each `runner.call` — natural seam in `agent_loop.rs:61`.
- `tool_call_started` always pairs with exactly one `tool_call_finished` (matched by `id` = the LLM's `tool_call_id`). Failures still emit a `tool_call_finished` but with `ok: false` and an `error` field.
- `assistant_text.final = true` marks the user-facing reply (the only kind of `assistant_text` v1 emits — intermediate "tool_calls only, no text" assistant messages are invisible in the UI by design; only the tool_calls themselves are shown).
- `result` is sent inline as a string. Already capped by `cap_or_refuse`. UI clamps display to ~10 lines with "show more."

### `/api/chat/history` shape

Returns `{ "events": [ ... ] }` — same union as the WS protocol, synthesized from the JSONL branch:

| JSONL entry | Event(s) emitted |
|---|---|
| `Role::User { content }` | `user_message { text: strip_envelope(content) }` |
| `Role::Assistant { content, tool_calls: Some }` | one `tool_call_started` per tool_call |
| `Role::Assistant { content, tool_calls: None }`, content non-empty | `assistant_text { text: content, final: true }` |
| `Role::Tool { content, tool_call_id }` | `tool_call_finished { id: tool_call_id, ok: true, result: content }` |

Lossy degradation: historical `tool_call_finished` always emits `ok: true`. The session JSONL doesn't record an explicit success/failure flag — only the result string. Live events get accurate `ok` from the agent loop. Acceptable for v1; if it bites we add a status field to `SessionEntry::Message` later. The current `content.is_empty()` filter (`main.rs:724`, `desktop/server.rs:196`) is dropped — tool-only assistant turns must surface.

### Module layout

**New file:**
- `agent/src/core/chat_events.rs` — `ChatEvent` enum (`#[derive(Serialize, Clone)] #[serde(tag = "type", rename_all = "snake_case")]`), `Sender = std::sync::mpsc::Sender<ChatEvent>`, and `try_send(events: Option<&Sender>, evt: ChatEvent)` helper that swallows `SendError` (closed channel = browser disconnected; agent loop continues).

**Modified — agent core:**
- `agent/src/core/mod.rs` — `pub mod chat_events;`.
- `agent/src/core/gateway.rs:45` — `chat()` gains `events: Option<&Sender<ChatEvent>>`. Forwards into `run_loop`. Emits `Done` / `Error` at exit.
- `agent/src/core/agent_loop.rs:28` — `run_loop()` and `execute_tool_calls()` gain `events`. Emits `ThinkingStarted` before `runner.call` (line 61), `ThinkingEnded` after. Inside `execute_tool_calls`: `ToolCallStarted` before `tools.execute` (line 363), `ToolCallFinished{ok: true}` on success, `ToolCallFinished{ok: false, error}` on cap-or-refuse / loop-detector block / cancellation. `LlmResponse::Text` (line 77) emits `AssistantText { final: true }` when content is non-empty.

**Modified — ESP32 transport (`agent/src/main.rs`):**
- `/ws/chat` handler (1174-1219): create `mpsc::channel::<ChatEvent>`. Spawn forwarder thread reading from rx, sending each event as a WS text frame via the detached sender. Spawn worker thread that calls `block_on(gw.chat(..., Some(&tx)))`. Handle inbound `{"type":"cancel"}` frames by calling `gw.cancel_chat(chat_id)`. (Note: `EspHttpWsConnection` only delivers one inbound frame per handler call — multi-frame inbound, needed for cancel mid-turn, is implemented by re-entry.)
- `/api/chat/history` (705-736): synthesize `{events:[...]}` from JSONL per the table above. Drop the `content.is_empty()` filter.

**Modified — desktop transport (`agent/src/desktop/server.rs`):**
- `handle_chat_ws` (492-532): same shape — spawn task that runs `gateway.chat` with a `tokio::sync::mpsc::Sender`; main task reads inbound frames and writes outbound from rx via `tokio::select!`.
- `api_chat_history` (176-213): synthesize `{events:[...]}`.

**Modified — web (`web/`):**
- `web/types/connection.ts` — `ChatEvent` discriminated union.
- `web/app/composables/useConnection.ts` — refactor `sendChatStream` to dispatch the new event types via `onEvent(evt: ChatEvent)`. Add `cancelChat()` that sends `{"type":"cancel"}` over the same WS. `getChatHistory` returns `{events: ChatEvent[]}`. Keep `sendChat` (REST) unchanged for headless callers.
- `web/app/pages/chat.vue` — replace text-only `parts` model with an event-driven timeline. Render `user_message` and `assistant_text` as bubbles; pair `tool_call_started`/`finished` by id into inline disclosure rows ("🔧 name — args (expand) → result"); show a "thinking..." pulse when bracketed by `thinking_*`; finalize on `done`/`error`; cancel button while sending.

## Verification

- `cargo build --no-default-features --features desktop` succeeds.
- `cargo test -p zenclaw-agent --no-default-features --features desktop` passes for changed modules (existing `agent_loop` + `gateway` tests must keep passing; no new tests for v1 — the units are integration-shaped).
- `just build devkitc` succeeds.
- Web typecheck passes.
- Manual smoke test against the desktop binary: send a message, observe sequenced events in the browser DevTools WS panel, confirm tool disclosures render, confirm reload via `/api/chat/history` reproduces the same disclosures.

## Default decisions

| Decision | Choice | Reasoning |
|---|---|---|
| Cancel | Yes — inbound `{"type":"cancel"}` calls `Gateway::cancel_chat` | Gateway already supports it. Once you can watch a long tool sequence, you'll want to stop runaway ones. |
| Tool result transport | Inline in WS frame | Already capped by `cap_or_refuse`. Avoids a separate "fetch result by id" endpoint. |
| Historical tool_call ok | Always `true` | JSONL doesn't record success/failure. Acceptable lossy degradation; live events still accurate. |
| Connection model | WS for web UI; REST kept for headless | `sendChat` callers (curl, scripts) keep working. |
| `assistant_text` granularity | Whole-message chunks | Token streaming deferred. Adding `assistant_text_delta` later doesn't change existing types. |
