# Multi-Conversation Web UI — Design Spec

**Date:** 2026-05-05
**Status:** Approved
**Scope:** v1 — Sub-project A. Foundational web UI for browsing and managing multiple chat sessions on the device. Prerequisite for cron-run history (Sub-project B) and any future feature that wants to surface per-session content in the web UI.

## Problem

The web UI today operates a single eternal conversation: `web/app/pages/chat.vue` is hard-coded to `chat_id = 'web'` (lines 143, 159). The backend already supports arbitrary `chat_id` strings — Telegram chats already coexist with the web chat under `data/sessions/`, and `SessionManager::list()` (`agent/src/core/sessions/mod.rs:572`) can enumerate them — but there is no UI to browse, switch between, rename, or delete them.

This is the first blocker for several features:

- **Cron run history** (Roadmap #3) — each run is naturally a chat session, but there is no place in the UI for them to surface.
- **Telegram visibility** — Telegram chats live in `data/sessions/` but are invisible in the web UI today.
- **Multiple parallel topics** — users cannot maintain separate threads (work / planning / experiments) in the same way they would on Claude.ai or ChatGPT.

Solving this once unblocks all three. The cron-run feature in particular becomes a thin tick-thread + "use the existing list" once the multi-conversation infrastructure exists.

## Non-Goals (v1)

- WebSocket-pushed sidebar updates (Telegram-arriving messages updating the web sidebar in real time without polling). Polling-on-focus + 30s background poll is acceptable.
- Pinning, starring, or archiving sessions.
- Soft-delete with restore window.
- Content search (searching message bodies, not just titles).
- Title-localization or user-configurable title prompts.
- Mobile-optimized sidebar gestures (swipe-to-delete, etc.); a basic responsive collapse is in scope.
- Migration / renaming of the existing `'web'` chat_id — it stays as a regular sidebar entry with a default-synthesized meta.

## User-Facing Decisions

The decisions made during brainstorming, kept here for traceability:

| # | Decision | Rationale |
|---|---|---|
| 1 | Sidebar layout (Claude.ai-style) — left column, conversation list, chat view on the right | Familiar pattern, scales beyond ~10 conversations, natural home for cron + Telegram surfacing |
| 2 | All sessions in one flat list — web + Telegram + future cron — differentiated by a kind icon | Simplest backend; web UI as control panel rather than peer-channel; can filter later if noisy |
| 3 | Explicit "New chat" button + `chat-{epoch_ms}` chat_ids | Discoverable affordance; timestamp IDs are stable, sortable, and free to generate |
| 4 | Sidebar row = title + kind icon + relative timestamp + 1-line preview; sorted by `last_activity_ms desc`; LLM-summarized title generated after first turn | Claude.ai parity; recency sort matches user mental model; LLM titles are pretty without being on the critical path |
| 5 | Rename + delete + search; URL is `/chat/<id>` (path param); existing `'web'` chat stays as a regular entry | Search at top of sidebar filters titles client-side; path-param URLs are RESTful and back-button-friendly; keeping `'web'` avoids a risky migration |

## Architecture

### Storage shape — per-session sidecar files

Each chat's metadata lives next to its JSONL: `data/sessions/<chat_id>.meta.json`. The cloud key is `sys/sessions/<chat_id>/meta.json`, sitting inside the existing per-chat prefix from `cloud_prefix(chat_id)` at `agent/src/core/sessions/mod.rs:226`.

```jsonc
// data/sessions/chat-1714914000000.meta.json
{
  "chatId": "chat-1714914000000",
  "kind": "web",                                 // "web" | "telegram" | "cron" | "other"
  "title": "Notes on tomato propagation",
  "titleSource": "llm",                          // "llm" | "user" | "firstMessage" | "default"
  "createdAtMs": 1714914000000,
  "lastActivityMs": 1714915800000,
  "lastMessagePreview": "…then I'd suggest air-layering instead.",
  "version": 1
}
```

Why sidecars (vs single index file or extending `SessionState`):

- **No contention.** Telegram-driven and web-driven writes touch different files, never lock each other.
- **Per-chat replication for free.** Each meta is inside its session's existing cloud prefix; the existing replicator handles it without new keys to plumb.
- **Self-healing on corruption.** A corrupt or missing meta file degrades to default-synthesized values; only the LLM-summarized title and user-renamed title are lost. Both rebuild via the existing flows.
- **No migration needed.** Existing chats appear in the sidebar from the moment this code ships, with synthesized defaults; metadata accretes as they receive activity.

### Backend module layout

```
agent/src/core/sessions/
  mod.rs                    ← MODIFIED: list_with_meta, set_meta, bump_activity, rename, delete
  meta.rs                   ← NEW: SessionMeta struct, SessionKind, TitleSource, detect_kind()

agent/src/core/cloud/
  boot.rs                   ← MODIFIED: per-chat restore loop also restores meta.json

agent/src/core/
  gateway.rs                ← MODIFIED: post-turn hook triggers maybe_generate_title()
  title_gen.rs              ← NEW: LLM background title generation (if extracted as own file)

agent/src/main.rs           ← MODIFIED: 4 new HTTP routes (/api/sessions {GET,POST,PATCH,DELETE})
agent/src/desktop/server.rs ← MODIFIED: same 4 routes for desktop target
```

### Frontend layout

```
web/app/
  composables/useSessions.ts      ← NEW: reactive sidebar state, optimistic updates
  components/SessionsSidebar.vue  ← NEW: list, search, new-chat, kebab menu
  layouts/chat.vue                ← NEW: two-column shell (sidebar + slot)
  pages/chat/[id].vue             ← NEW: dynamic route, contents lifted from current chat.vue
  pages/chat/index.vue            ← NEW: bare /chat — redirects to most-recent chat,
                                          or shows empty pane if none exist
  pages/chat.vue                  ← REMOVED (logic moved to pages/chat/[id].vue)
```

Routing:

| URL | Page | Behavior |
|---|---|---|
| `/chat/<id>` | `pages/chat/[id].vue` | Render the chat with that id; sidebar highlights matching row |
| `/chat` | `pages/chat/index.vue` | If sessions exist, redirect to the most-recent (highest `lastActivityMs`); else show empty pane with "Click 'New chat' to start" |
| `/` | (existing `pages/index.vue`, unchanged) | Landing page |

## Components

### `agent/src/core/sessions/meta.rs` (new, ~150 lines)

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind { Web, Telegram, Cron, Other }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleSource { Llm, User, FirstMessage, Default }

#[derive(Serialize, Deserialize, Clone, Debug)]
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

impl SessionMeta {
    /// Classify by chat_id pattern. Stable rules — see tests for the
    /// exhaustive matrix; this is a forward-compat trap if changed.
    pub fn detect_kind(chat_id: &str) -> SessionKind { /* ... */ }

    /// Build a sensible default for a chat that has no sidecar yet.
    /// Optionally peeks at the first JSONL entry to derive a title for
    /// chats that already have content (e.g., the legacy `'web'` chat
    /// the first time this code ships, or any chat whose meta sidecar
    /// was lost). When no JSONL is provided, falls back to a plain
    /// "New chat" placeholder with `TitleSource::Default`.
    pub fn synthesize_default(
        chat_id: &str,
        now_ms: u64,
        first_user_message: Option<&str>,
    ) -> Self { /* ... */ }
}

fn default_version() -> u32 { 1 }
```

`detect_kind` rules. The function must accept both the *canonical* chat_id (e.g., `cron:job-abc:run-1`) and the *sanitized* form (e.g., `cron_job-abc_run-1`) because `safe_chat_id` at `sessions/mod.rs:93-95` translates `:` → `_` on disk. When a session's meta is missing and `list_with_meta()` synthesizes from the directory listing, only the sanitized form is available.

| Pattern | Kind |
|---|---|
| `chat_id == "web"` or `chat_id.starts_with("chat-")` | `Web` |
| `chat_id` parses as a positive integer | `Telegram` |
| `chat_id.starts_with("cron:")` or `chat_id.starts_with("cron_")` | `Cron` |
| Anything else | `Other` |

Sub-project B will define its canonical cron chat_id format. For Sub-project A, this rule is forward-compat insurance — web (`chat-{ts}`, `web`) and Telegram (numeric) chat_ids contain no special characters, so sanitization is a no-op and the canonical form survives the round-trip.

### `agent/src/core/sessions/mod.rs` (extended)

| New method | Behavior | Errors |
|---|---|---|
| `meta(chat_id) -> Option<SessionMeta>` | Read sidecar from local FS first; missing returns `None` (caller may synthesize via `synthesize_default`, optionally passing the first user-turn from the JSONL for a `FirstMessage`-source title) | IO error → `Err` |
| `set_meta(chat_id, meta) -> io::Result<()>` | Cloud-aware: `cache.put` → `strict_put(sys/sessions/<id>/meta.json)` → local fs write. Same pattern as `core/cron.rs:264-288` | Strict-put failure returns `Err`; no in-memory state to roll back |
| `list_with_meta() -> Vec<SessionMeta>` | Walk `sessions_dir`. For each `<id>.jsonl`: read sidecar if present; otherwise read the first JSONL entry, extract the user-turn text, pass to `synthesize_default(chat_id, mtime, Some(first_msg))`, persist the synthesized meta back to disk. Orphan `<id>.meta.json` files (no matching JSONL) are skipped. | Per-session errors logged + skipped; never aborts whole list |
| `bump_activity(chat_id, preview)` | Read meta (synthesize if missing) → set `last_activity_ms = now`, `last_message_preview = truncate(preview, 120)` → write | Best-effort; logged but does not fail the chat turn |
| `rename(chat_id, title)` | Validate `1 <= len(title) <= 80` after trim. Read meta → set `title`, `title_source = User` → write | Returns `Err(InvalidInput)` on validation; `Err` on IO/cloud failure |
| `rename_internal(chat_id, title, source)` | Same as `rename` but bypasses validation and accepts any `TitleSource`. Used by the LLM title task. | IO/cloud failure → `Err` |
| `delete(chat_id)` | Remove `<id>.jsonl` + `<id>.meta.json` locally. Walk `sys/sessions/<id>/` in cloud and delete each key (mirrors existing `clear()` cloud-cleanup pattern). | Surfaces partial failure; caller (HTTP layer) returns 500 |

### HTTP routes

Both `agent/src/main.rs` (ESP32) and `agent/src/desktop/server.rs` (desktop) gain identical routes. Per `feedback_symmetric_platform_status.md`, behavior is identical.

| Method | Path | Body | Response | Status |
|---|---|---|---|---|
| `GET` | `/api/sessions` | — | `[SessionMeta, ...]`, sorted desc by `lastActivityMs` | 200 / 500 |
| `POST` | `/api/sessions` | — | `{chatId, meta}` | 201 / 500 |
| `PATCH` | `/api/sessions/:id` | `{title: string}` | updated `SessionMeta` | 200 / 400 / 404 / 500 |
| `DELETE` | `/api/sessions/:id` | — | empty | 204 / 404 / 500 |

### Title generation task

```rust
// agent/src/core/title_gen.rs (or inline in core/mod.rs)
pub async fn maybe_generate_title(
    chat_id: &str,
    sessions: Arc<SessionManager>,
    runner: Arc<dyn Runner>,
    config: Arc<Config>,
) {
    // Bail if title_source already User or Llm.
    // Fetch last 4-6 entries from session history.
    // Run a one-shot LLM call: "Summarize this conversation in 6 words or fewer."
    // On success: sessions.rename_internal(chat_id, response, TitleSource::Llm).
    // On any failure: log warn, leave title as-is.
}
```

Triggered from `Gateway::chat_with_events` *after* the assistant's `Done` event has been sent, only when `meta.title_source` is `FirstMessage` or `Default` (or meta is missing). Uses `block_on` on ESP32 inside the post-completion hook in `agent_thread`; uses `tokio::spawn` on desktop. **Off the user's critical path** — the user has already received their reply.

### `agent/src/core/cloud/boot.rs` (extended)

The per-chat restore loop currently downloads `base.jsonl` + `log-NN.jsonl`. Add `meta.json` to that list. Bytes-for-bytes restore, same defensive layers. ~5-line change.

### Frontend components

**`composables/useSessions.ts`** — owns reactive sidebar state and optimistic update logic.

```ts
export const useSessions = () => {
  const sessions = ref<SessionMeta[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function refresh()
  async function create(): Promise<SessionMeta>
  async function rename(id: string, title: string)
  async function remove(id: string)
  function bumpLocal(id: string, preview: string)

  // Auto-refresh hooks: window 'focus' event + 30s setInterval
  // Optimistic rename/delete: snapshot → mutate → on failure restore + toast

  return { sessions, loading, error, refresh, create, rename, remove, bumpLocal }
}
```

**`components/SessionsSidebar.vue`** — visual:

- Top: "New chat" button (full-width primary) and search input below it
- Scrollable list, one row per session, sorted by `lastActivityMs desc`
- Row: kind icon (chat / paper-plane / clock) + title (editable in place on rename) + relative timestamp + 1-line truncated preview
- Active row highlighted via `useRoute().params.id` match
- Kebab menu per row: Rename (in-place edit) / Delete (UModal confirm)
- Empty state: "No conversations yet — click 'New chat' to start"
- Search empty state: "No conversations match `<query>` · Clear search to see all"
- Error banner above the list when `/api/sessions` GET fails

**`pages/chat/[id].vue`** — refactor of current `chat.vue`:

- Reads `route.params.id` instead of hardcoded `'web'`
- Watches `route.params.id` to swap conversations on click
- Calls `useSessions().bumpLocal(id, preview)` after each send and reply

**`pages/chat/index.vue`** — bare `/chat`:

- On mount, if any sessions exist, redirect to the most-recent chat (`router.replace(\`/chat/${mostRecent.chatId}\`)`)
- Otherwise show an empty pane with "Click 'New chat' to start"
- Renders inside the `chat.vue` layout, so the sidebar is visible regardless

**`layouts/chat.vue`** — two-column flex shell:

- Sidebar left (300px desktop, collapsible drawer on mobile)
- `<NuxtPage />` right
- Used by both `pages/chat/index.vue` and `pages/chat/[id].vue`

## Data Flow

### Boot (cold start)

```
Device powers on
 ├─ NIC up, mDNS announce
 ├─ Cloud boot_restore (cloud/boot.rs)
 │   └─ for each "sys/sessions/<id>/" prefix in S3:
 │        download base.jsonl, log-NN.jsonl, meta.json  ← NEW
 │        write to data/sessions/<id>.{jsonl,meta.json}
 ├─ HTTP server up
 └─ Web client → GET /api/sessions
       └─ list_with_meta() → JSON sorted desc by lastActivityMs
```

### New-chat creation

```
User clicks "New chat"
 ├─ POST /api/sessions
 │   ├─ chat_id = "chat-{epoch_ms()}"
 │   ├─ meta = synthesize_default(chat_id, now, None): kind=Web,
 │   │       title="New chat", titleSource=Default, lastActivityMs=now, preview=""
 │   ├─ set_meta() — cache → strict_put → fs.write
 │   └─ 201 {chatId, meta}
 ├─ optimistic prepend to sessions.value
 └─ router.push(`/chat/${chatId}`)

User submits first message
 ├─ POST /api/chat {chatId, message}
 ├─ Gateway::chat_with_events runs the turn
 │   └─ JSONL appended (user + assistant turns)
 ├─ bump_activity(chatId, last assistant text truncated)
 ├─ POST returns assistant text → bumpLocal in client
 │   (sidebar entry moves to top, preview updates instantly)
 └─ post-completion hook (background):
       title_source == Default → maybe_generate_title()
        ├─ runner.chat([…last 4 entries…], "Summarize in 6 words.")
        └─ on success: rename_internal(chatId, "Tomato propagation", Llm)
       next sidebar refresh shows the new title
```

### Rename (optimistic with rollback)

```
User edits title in sidebar row → blur or Enter
 ├─ snapshot = sessions[idx].title
 ├─ sessions[idx].title = newTitle  (optimistic)
 ├─ PATCH /api/sessions/<id> {title: newTitle}
 │   ├─ validate length 1..=80
 │   ├─ rename(id, title) — sets titleSource=User
 │   └─ 200 / 400 / 404 / 500
 └─ on 4xx/5xx: sessions[idx].title = snapshot; toast
```

### Delete (with cloud cleanup)

```
User opens kebab → "Delete chat" → UModal confirm
 ├─ snapshot = sessions[idx]
 ├─ sessions.splice(idx, 1)  (optimistic)
 ├─ if route.params.id == id: router.push('/chat')
 ├─ DELETE /api/sessions/<id>
 │   ├─ delete() — remove jsonl + meta locally
 │   ├─ for each cloud key under sys/sessions/<id>/: cloud_store.delete()
 │   └─ 204
 └─ on 4xx/5xx: sessions.splice(idx, 0, snapshot); toast; navigate back
```

### Telegram-driven sidebar update

```
Telegram user sends "/ping"
 ├─ agent_thread polls Telegram → message with chat_id = "987654321"
 ├─ Gateway::chat("987654321", "/ping", "telegram")
 │   ├─ if no meta exists: set_meta(synthesize_default(chat_id, now, None))
 │   │      kind=Telegram, title="Telegram 987654321"
 │   ├─ run turn, append entries
 │   └─ bump_activity("987654321", preview)
 └─ Web client (focused elsewhere):
      next /api/sessions refresh (focus or 30s poll) shows the row
      bumped to top with fresh preview + timestamp
```

## Error Handling

### Backend write paths

| Failure | Path | Behavior |
|---|---|---|
| `set_meta` cloud strict-put exhausts retries | `POST /api/sessions`, `PATCH /api/sessions/:id` | 500 with `{error: "..."}`. UI reverts optimistic change + toast |
| `set_meta` cloud succeeds, local fs.write fails | same | 200 (cloud is canonical). Log warning. Boot-restore re-syncs from cloud |
| `bump_activity` failure | every successful chat turn | Log warning, **do not propagate** — chat reply already sent; sidebar staleness is non-critical |
| `delete` partial cloud failure (local cleared, S3 keys remain) | `DELETE /api/sessions/:id` | 500 with details. **Risk:** next boot-restore could resurrect leftover keys. Mitigation v1: log loudly; user re-deletes if observed. Tombstone protocol deferred to v2 |
| Title generation LLM call fails | post-turn background task | Log warning, leave title as-is. Next turn-completion that satisfies the trigger condition retries |

### Validation

- `PATCH` body `title`: trim, then `1..=80` chars. Empty / too-long → 400. UI shows inline error, does not apply.
- `PATCH`/`DELETE` against missing chat_id → 404. UI removes the stale row.
- `POST` chat_id collision (rapid same-millisecond clicks): server sleeps 1ms and retries once; second collision → 500.

### Concurrent writes

A `PATCH` rename and a Telegram-arrived-message `bump_activity` can race the same `<id>.meta.json`. They update **different fields** (title vs preview/activity); last-writer-wins is acceptable. **No locking added in v1.** Revisit if a future feature stores overlapping fields.

### Boot-restore inconsistencies

| State | Behavior |
|---|---|
| `<id>.jsonl` present, `<id>.meta.json` missing | Synthesize default on the fly. Logged INFO. Persisted on next `bump_activity` |
| `<id>.meta.json` present, `<id>.jsonl` missing | Skipped from sidebar. Logged WARN. Cleanup deferred to manual GC |
| `<id>.meta.json` parse error (corruption) | Logged WARN. Synthesize default. Corrupt file overwritten on next `set_meta`. Self-healing |
| Cloud meta.json 404 during restore | Skip, restore other keys normally. Existing per-key resilience |

### Frontend errors

- `GET /api/sessions` failure → red banner at top of sidebar with explicit retry button (no auto-retry loop).
- Stale row click (chat deleted server-side between refresh and click) → `chat/[id].vue` fetches history, shows "This conversation no longer exists · Back to list". Sidebar refresh on focus removes the row.
- Search with no matches → empty state with "Clear search to see all" hint.
- WebSocket disconnect mid-chat → existing `chat.vue` reconnect logic handles this; **no new error path** from this work.

### ESP32-specific budget

- Heap during meta write: ~300 bytes/meta. The expensive part (TLS handshake ~40-50KB) is the existing `strict_put` machinery — adding meta writes piggybacks on the same path as cron + memory.
- Flash wear: 50 turns/day × 300 bytes × 5 active chats ≈ 75KB/day. LittleFS wear-levels this. Two orders of magnitude below concern threshold.

## Testing

### Backend unit tests

**`agent/src/core/sessions/meta.rs`** — inline `#[cfg(test)] mod tests`:

- `detect_kind` exhaustive matrix:
  - canonical: `web`, `chat-1714914000000`, `987654321`, `cron:job-abc:run-1`, `custom-thing`
  - sanitized: `cron_job-abc_run-1` (after `:` → `_`) → still `Cron`
- `synthesize_default_with_first_message` — title derived from truncated user turn, `titleSource = FirstMessage`
- `synthesize_default_without_first_message` — title is "New chat", `titleSource = Default`
- `meta_serde_roundtrip` — serialize, deserialize, equal
- `version_field_round_trips` — schema-evolution insurance

**`agent/src/core/sessions/mod.rs`** — extends existing inline tests:

- `list_with_meta_synthesizes_when_missing` — meta absent; chat_id reflected, default title
- `list_with_meta_synthesizes_from_first_message_when_jsonl_has_content` — non-empty JSONL produces a `FirstMessage` title
- `list_with_meta_persists_synthesized_meta` — first call writes the synthesized meta to disk so subsequent calls are O(1)
- `list_with_meta_skips_orphan_meta`
- `bump_activity_updates_last_activity_and_preview`
- `bump_activity_truncates_preview_to_120_chars`
- `rename_sets_title_source_user`
- `rename_validates_title_length`
- `rename_internal_accepts_any_source`
- `delete_removes_jsonl_and_meta_locally`
- `corrupt_meta_falls_back_to_default`

**Cloud-aware paths** — reuse the `FakeStore` pattern from `agent/src/core/cron.rs:719-749`:

- `set_meta_cloud_succeeds_writes_local`
- `set_meta_cloud_fails_returns_err_no_local_write`
- `set_meta_cloud_succeeds_local_fails_logs_warns`
- `delete_walks_session_prefix_in_cloud`

**Boot-restore extension** — extends `agent/src/core/cloud/boot.rs` tests:

- `restore_includes_meta_json` alongside existing `base.jsonl` + `log-NN.jsonl` assertions

### HTTP route tests

**Desktop** (`agent/src/desktop/server.rs`) — using axum's `tower::ServiceExt::oneshot`:

- `get_sessions_returns_sorted_array`
- `post_sessions_creates_meta_returns_201`
- `patch_sessions_validates_title_length` (400 on empty / >80 chars)
- `patch_sessions_404_on_missing_id`
- `delete_sessions_removes_all_layers` (with FakeStore)

**ESP32** — no automated route tests (consistent with rest of `main.rs`); exercised manually via curl in the smoke checklist below.

### Frontend tests

**Component (Vitest)**:

- `SessionsSidebar.test.ts` — search filter, click-to-select emits, optimistic delete + revert on 500
- `useSessions.test.ts` — composable optimistic update logic, rollback paths

**End-to-end (Playwright MCP)** — happy-path script in `docs/superpowers/playbooks/`:

1. Open dashboard, connect to device hostname
2. Click "New chat" → URL becomes `/chat/<id>`
3. Send "ping" → reply arrives → sidebar row appears
4. Wait ~5s → LLM title replaces truncated first message
5. Click row title → edit "Custom title" → blur → reload → persists
6. Kebab → Delete → confirm → row disappears, URL navigates back

Run on demand against DevKitC + Guition P4. Not in CI (Playwright MCP is interactive).

### Manual smoke checklist (post-flash)

```bash
HOST=zenclaw-<name>.local

# 1. Create + first message
ID=$(curl -sf -X POST http://$HOST/api/sessions | jq -r .chatId)
curl -sf -X POST http://$HOST/api/chat -H 'Content-Type: application/json' \
  -d "{\"chat_id\":\"$ID\",\"message\":\"ping\"}"

# 2. Verify meta exists, title eventually populates
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\")"
sleep 5
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\") | .titleSource"  # expect "llm"

# 3. Rename
curl -sf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' -d '{"title":"smoke-test"}'

# 4. Delete + cloud cleanup verify
curl -sf -X DELETE "http://$HOST/api/sessions/$ID"
curl -sf "http://$HOST/api/cloud/files?prefix=sys/sessions/$ID/"  # → empty
```

Run on DevKitC and Guition P4. Step 4 catches the partial-delete risk.

### Acceptable gaps (out of v1 scope)

- **Load testing** (50+ concurrent sessions / sidebar refreshes). Sidecar pattern already addressed at design time; meta-cache file is a v2 add-on.
- **Fuzz testing meta.json parser** — relies on serde robustness.
- **Network-fault injection** — `strict_put` already has retry/exhaust tests at the layer below.
- **Title-prompt evaluation** — LLM is non-deterministic; we test that the call is dispatched, not what it returns.

## Open Questions

None blocking. Future-work items folded into Non-Goals.

## Future Work

- **Sub-project B (Cron execution)** — once this lands, cron tick spawns a small thread that sends `ChatRequest`s with `chat_id = cron:<job>:run-<ms>` into the existing chat channel. Each run becomes a sidebar entry naturally. No new persistence layer.
- **Real-time push** — `/ws/sessions` with `session_updated` events. Telegram-arriving messages would update the web sidebar without poll lag.
- **Tombstone protocol for delete** — `sys/sessions/<id>/.deleted` marker file that boot-restore checks before restoring. Closes the partial-cloud-delete resurrection risk.
- **Content search** — server-side endpoint that scans JSONL bodies. Required if title-only search proves insufficient.
- **Pin / archive** — `pinned: bool` and `archived: bool` fields on `SessionMeta`. Sidebar gets pinned-first ordering and a collapsed "Archived" section.
- **Telegram identity enrichment** — Telegram chats currently get titles like `Telegram 987654321`. Calling `getChat` once per chat to fetch the user's first name produces nicer titles.
- **Meta cache file** — if N grows large enough that listing N sidecar reads becomes a bottleneck, add a single `data/sessions/.cache.json` that mirrors the latest list. Source of truth stays in sidecars; the cache is rebuildable.
