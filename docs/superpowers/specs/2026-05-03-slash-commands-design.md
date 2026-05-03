# Slash Commands — Design Spec

**Date:** 2026-05-03
**Status:** Approved
**Scope:** v1 — minimal user-facing slash commands across Telegram and web channels

## Problem

ZenClaw bots running over Telegram show a `/` command menu (`/status`, `/help`, etc.) but no code in the repo implements slash-command parsing or registers commands with Telegram. Investigation revealed:

1. The Telegram menu was registered manually via @BotFather and is not code-owned. It can drift out of sync with what the bot actually supports.
2. When a user taps `/status`, the text reaches `gateway::chat()` verbatim and is interpreted by the LLM, which heuristically dispatches to a tool (e.g., `session(action="status")`). Output is non-deterministic and exhibits bugs the LLM hides — for example, `session_tools.rs` reports `platform: unknown` on ESP32 because its `cfg!(target_os = ...)` ladder doesn't include `espidf`, and the LLM serenely formats the broken value into a markdown table.
3. The web UI has no slash commands at all.

We want a code-owned slash-command layer that:

- Provides deterministic, fast (no LLM round-trip) responses for a small set of common commands.
- Registers the same commands with Telegram via `setMyCommands` on every boot, eliminating drift.
- Works identically on both Telegram and the web chat — single parser, single execution path, no per-channel duplication.
- Falls through to the existing LLM path for anything unrecognized (backwards compatible).

## Non-Goals

- Per-channel command variants (e.g., `/restart` Telegram-only).
- Multi-session-per-channel infrastructure or chat picker UI.
- `/model <name>` switching, `/memory` viewer, or other tool-shortcut commands.
- Confirmation prompts for destructive operations.
- Web autocomplete dropdown when user types `/`.
- Localization of command descriptions.

## v1 Command Set

Four commands. Two of them are aliases for the same operation, kept because both names are intuitive:

| Command | Action |
|---|---|
| `/new` | Alias for `/clear`. |
| `/clear` | Wipe `data/sessions/{chat_id}.jsonl`. Drop any cached message history in `SessionState`. **Preserve** `model_override` and other non-history settings on `SessionState` — the user's model fast-dial survives a clear. May require a new `SessionManager::clear_history(chat_id)` method that wipes JSONL + cached entries while leaving the rest of `SessionState` intact. |
| `/status` | Render a markdown table of live device facts (hostname, IP, link, heap, RSSI, uptime, model, session size). Pulls from real platform sources, fixing the `platform: unknown` bug. |
| `/help` | Static markdown bullet list of available commands and what they do. |

## Architecture

```
agent/src/core/
  commands.rs              ← NEW. Parser, executors, menu list (single source of truth).
  gateway.rs               ← MODIFIED. Intercept in chat_with_events before compaction.
  channels/telegram.rs     ← MODIFIED. Add Poller::set_my_commands.
desktop/run.rs             ← MODIFIED. spawn_telegram_loop calls set_my_commands once on startup.
main.rs                    ← MODIFIED. ESP32 telegram_thread calls set_my_commands once on startup.
```

### `commands.rs` public surface

```rust
pub enum Command { New, Clear, Status, Help }

pub fn parse(text: &str) -> Option<Command>;

pub async fn execute(
    cmd: Command,
    chat_id: &str,
    channel: &str,
    sessions: &SessionManager,
    config: &AgentConfig,
    runtime: &RuntimeFacts,
) -> String;

pub fn menu() -> &'static [(&'static str, &'static str)];
//                          ^name        ^description (used by Telegram setMyCommands)
```

`menu()` returns a `const` slice. The same slice is consumed by both `setMyCommands` (display) and `parse()` (dispatch) — drift is impossible by construction.

### `RuntimeFacts`

```rust
pub enum LinkKind {
    Wifi { ssid: String, rssi: Option<i32> },
    Ethernet,
    Desktop,
}

pub struct RuntimeFacts {
    pub hostname: String,
    pub ip: Option<String>,
    pub link: LinkKind,
    pub free_internal_heap: Option<u32>, // bytes; ESP32 only
    pub free_psram: Option<u32>,         // bytes; ESP32 only
    pub uptime_secs: u64,
    pub agent_name: String,
    pub platform: &'static str,          // "esp32-s3" | "esp32-p4" | "linux" | "macos" | "windows"
    pub session_bytes: u64,              // file size of {chat_id}.jsonl
    pub session_entries: usize,          // line count
    pub model: String,                   // resolved: model_override OR config default
}
```

A pure data struct — easy to populate in tests, easy to extend (add a field, both call sites either fill it or set `None`). The `platform` field replaces the broken `cfg!(target_os = ...)` ladder in `session_tools.rs::do_status` — that bug is the whole reason this design exists.

Population sites:

- **ESP32 `main.rs`** — passes mDNS handle, NIC handle, hostname, IP into `Gateway`. New helper `Gateway::runtime_facts(chat_id)` reads them and adds session/model values it already owns. Heap via `esp_idf_svc::sys::esp_get_free_heap_size()` and `heap_caps_get_free_size(MALLOC_CAP_SPIRAM)` — same calls used by the existing `/api/status` handler.
- **Desktop `run.rs`** — populates the same struct with desktop-shaped values: `link = LinkKind::Desktop`, heap fields = `None`, hostname from config or `"desktop"`.
- **Tests** — populate directly. No IO needed.

A trait-based alternative (`runtime_facts() -> impl RuntimeFactsProvider`) was considered and rejected — over-abstracted for one consumer.

### Hook point in `gateway.rs`

Slash-command interception happens at the **top of `chat_with_events`** (`gateway.rs:134`), specifically:

- **Before** the auto-compaction call at `gateway.rs:159`, so `/clear` doesn't trigger summarization right before wiping.
- **After** the active-chat cancellation flag setup at `gateway.rs:142–150`, so a slash command still cancels any in-flight LLM turn on the same `chat_id`.

When `parse()` returns `Some(cmd)`:

1. Build `RuntimeFacts` for the current `chat_id`. (Cheap — only `Status` actually consults it; `New`/`Clear`/`Help` ignore it. We build eagerly anyway because `RuntimeFacts` construction is well under 1ms on ESP32 — file size + heap reads are O(1) syscalls — and eager construction keeps `execute()`'s signature uniform across commands.)
2. Call `commands::execute(cmd, chat_id, channel, sessions, config, &runtime)`.
3. If an `EventSender` is present, emit `assistant_text { text }` and `done`.
4. Return the string to the caller.

Skip the system-prompt build, tool definitions, runner dispatch, and tool loop entirely.

`execute()` is declared `async fn` even though all v1 operations are synchronous. Rationale: it's called from `chat_with_events` (already `async`), and forward-compat commands like `/restart` or `/model` may need async I/O (NVS write, HTTP). Making it async now avoids a future API break.

### `Poller::set_my_commands` (Telegram menu sync)

```rust
impl Poller {
    pub async fn set_my_commands(
        &self,
        http: &dyn HttpClient,
        commands: &[(&str, &str)],
    ) -> Result<(), Error> {
        // POST https://api.telegram.org/bot<token>/setMyCommands
        // body: {"commands":[{"command":"new","description":"..."}, ...]}
    }
}
```

Called once at poller startup, **before** the `poll_once` loop:

- `desktop/run.rs::spawn_telegram_loop` — right after `Poller::new()`, before the producer task spawn.
- `main.rs` ESP32 telegram thread — same position relative to the `poll_once` loop.

`setMyCommands` is idempotent. Calling on every boot with the same payload is fine and gives self-healing if BotFather state drifts.

## Channel Behavior

### Telegram
The user types `/status` (or taps it in the menu). Telegram forwards the text to the bot. `Poller::poll_once` returns it as `IncomingMessage { chat_id, text: "/status" }`. The consumer calls `gateway.chat(&chat_id, &text, "telegram")`, which routes to `chat_with_events`. The slash-command interceptor matches, executes, and returns the string. The Telegram consumer calls `channel.deliver(chat_id, &reply)` exactly as it does for LLM replies.

In group chats, Telegram appends the bot's username (`/status@zenclaw_bot`). The parser strips this suffix before matching. Standard bot behavior.

### Web (REST `/api/chat`)
Returns the deterministic string in the response body. Frontend renders it like any assistant text. Zero frontend changes.

### Web (WS `/ws/chat`)
Emits `assistant_text { text: "..." }` followed by `done` — same shape `chat.vue::applyEvent` already handles. Zero frontend changes.

## Edge Cases

| Input | Behavior |
|---|---|
| `/new`, `/clear`, `/status`, `/help` | Match — execute deterministically, skip LLM and skip auto-compaction. |
| `/new extra trailing text` | Match — trailing args ignored (none of v1's commands take args). |
| `/foo` (unknown command) | **Fall through to LLM.** Sent verbatim as user message. Existing behavior. |
| `/` alone, `/  ` (just slash + whitespace) | Fall through to LLM. Not worth special-casing. |
| `not a command /clear` | Fall through. Parser only matches when the message *starts* with a recognized command. |
| `/status@zenclaw_bot` | Strip `@<botname>` suffix before matching. |
| Empty message `""` | Existing gateway behavior — empty-message error. Not a slash-command concern. |

Fall-through is the right default for two reasons:

1. **Backwards compatibility** with users whose habits or BotFather menus predate this PR — hard-rejecting `/foo` would break them.
2. **Forward compatibility** — when we later add `/memory` or `/model`, no migration is needed.

## Error Handling

Slash-command execution must not fail silently.

- **`/clear` filesystem errors**: if removing the JSONL fails for any reason except `NotFound`, return `"Failed to clear session: <error>"`. Do **not** proceed to wipe in-memory state — that would create a phantom-success situation where the file persists but state was dropped.
- **`/status` partial info**: if a `RuntimeFacts` field can't be read (e.g., RSSI on Ethernet), render `—` for that row. Never abort the whole status output on a single missing field.
- **`setMyCommands` boot failure**: log a warning and continue. Telegram occasionally rate-limits; not having an updated menu is degraded UX, not a broken bot.

## Testing

### Unit tests in `commands.rs`

| Test | Asserts |
|---|---|
| `parse_recognizes_all_four_commands` | `parse("/new")`, `/clear`, `/status`, `/help` return `Some(_)`. |
| `parse_strips_telegram_botname_suffix` | `parse("/status@zenclaw_bot")` matches `Status`. |
| `parse_returns_none_for_unknown` | `parse("/foo")` and `parse("hello")` return `None`. |
| `parse_ignores_trailing_args` | `parse("/clear extra")` matches `Clear`. |
| `execute_clear_deletes_session_file` | Pre-seed JSONL, run command, assert file gone. |
| `execute_clear_preserves_model_override` | Pre-seed `SessionState { model_override: Some("x") }`, clear, assert override survives. |
| `execute_status_renders_with_partial_facts` | `RuntimeFacts` with `rssi=None`, `psram=None` — output contains `—` rows. |
| `menu_list_matches_parse_table` | Every name in `menu()` is also recognized by `parse()` (drift guard). |

### Telegram poller tests
Reuse the recorded-HTTP-mock pattern at `telegram.rs:227+`. One new test asserting `Poller::set_my_commands` posts the right URL, the right body shape, and handles a non-200 response without panicking.

### End-to-end smoke (manual, post-firmware-reflash)
1. `/help` in Telegram → markdown bullet list shows.
2. `/status` in Telegram → real device facts, including `platform: esp32-s3` (not `unknown`).
3. `/clear` in Telegram → session wiped; next message starts fresh history.
4. Repeat the three in the web chat (`http://<hostname>.local` → Chat tab) and verify `assistant_text` events render.
5. Open BotFather → `/mybots` → `Edit Bot` → `Edit Commands` → confirm the menu matches what `commands::menu()` returns.

## Non-Functional Notes

- Slash commands sidestep the multi-tool-call splitting pitfall documented in `CLAUDE.md` because they never enter the LLM loop. No `tool_call_id` matching, no `extra_content` round-trip, no compaction interference.
- ESP32 firmware needs a rebuild and reflash (per `feedback_wizard_firmware_rebuild.md`): `./scripts/build-rust-firmware.sh devkitc` (and `guition-p4` if testing P4) before user verification.
- Symmetry rule (per `feedback_symmetric_platform_status.md`): the *shape* of `RuntimeFacts` is the same on ESP32 and desktop; per-platform fields are `Option<T>` rather than feature-gated. Behavior of the slash commands is identical across channels.

## File Manifest

**New files:**
- `agent/src/core/commands.rs`
- (No new web/desktop frontend files — web UI changes are zero in v1.)

**Modified files:**
- `agent/src/core/mod.rs` — add `pub mod commands;`
- `agent/src/core/gateway.rs` — slash-command interception in `chat_with_events`; add `runtime_facts(chat_id)` helper.
- `agent/src/core/sessions/...` — likely add `SessionManager::clear_history(chat_id)` (wipes JSONL + cached entries, preserves `model_override` and other `SessionState` fields).
- `agent/src/core/channels/telegram.rs` — add `Poller::set_my_commands` + tests.
- `agent/src/desktop/run.rs` — call `set_my_commands` on poller startup.
- `agent/src/main.rs` — call `set_my_commands` on ESP32 telegram thread startup; populate `RuntimeFacts` source values into `Gateway` at construction.

## Out of v1 Scope (Tracked Follow-ups)

- `GET /api/commands` endpoint serving `commands::menu()` as JSON, for a future `/` autocomplete in `chat.vue`.
- `/memory`, `/restart`, `/model <name>` — set B from brainstorming. Add later by appending to `menu()` and growing the `Command` enum.
- Confirmation modals for `/clear` (web) and inline-keyboard confirms (Telegram).
- Multi-session per channel (web chat picker rotating `chat_id`).
