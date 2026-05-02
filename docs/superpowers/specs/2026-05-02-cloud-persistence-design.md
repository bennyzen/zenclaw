# Cloud Persistence — Design

**Date**: 2026-05-02
**Status**: Approved (brainstorming complete; awaiting implementation plan)
**Roadmap item**: README §Roadmap #1 — *Cloud persistence — automatic write-through replication*
**Supersedes**: the aspirational claims in the README's *Cloud Persistence* section, which currently describe the design below as if it were already shipping.

---

## 1. Problem

ESP32 SPIFFS is wear-prone. Filesystem corruption from power loss, firmware reflashes, or flash wear is a real risk for an agent that writes per-conversation-turn. Today, all of `data/` (sessions, MEMORY.md, cron, identity files, user uploads) lives only on local flash, and an `S3-compatible storage` tool exists but is invocable only by explicit LLM call.

**User goal** (from the brainstorming session):

> "These devices are brittle and we risk to lose the work of the user on failure. The device should do auto-restore and auto-sync to S3. This feature is only active when configured, but the configuration should be strongly encouraged by the UI. If configured, reads/writes should happen from the storage, avoiding flash wear and allowing real work to be done."

Plus an explicit device-swap requirement:

> "It should be possible for the user to swap device and fully recover its whole work and configuration from S3 storage."

## 2. Goals

1. **Survive device death**: a fresh device, given the same bucket creds, fully recovers the previous device's work.
2. **Avoid flash wear**: when cloud is configured, the per-turn write path does not touch flash.
3. **Keep the agent working offline**: network drops degrade gracefully; the agent stays responsive.
4. **Bound boot-time risk**: no single oversized object can brick a device on next boot.
5. **Strongly encourage configuration in the UI**, but never make it mandatory — the agent runs fine without cloud.
6. **Bucket-per-device**, no in-bucket prefix sharding — bucket itself is the device's cloud identity.

## 3. Non-goals (firm — out of scope for v1)

| Item | Reason |
|---|---|
| Multi-device write coordination (CRDT / conflict-free replication) | Bucket-per-device makes it unnecessary by design |
| Encryption-at-rest beyond what R2/B2 provides | Passphrase encryption explicitly declined; trust private-bucket isolation |
| Versioning / point-in-time recovery | User can enable R2 versioning bucket-side if desired |
| Compression | Working set is small; bandwidth is rare; <50% savings on JSONL |
| Multiple bucket targets (sharded storage) | Undoes the simplicity wins of bucket-per-device |
| Auto-rotation of S3 access keys | User-managed via R2 dashboard |
| Real-time S3 webhooks (notify device of bucket changes) | Single-writer model — not needed |
| Reading user files into the session prompt verbatim | The new `read_range`/`head`/`tail` actions force chunked reads exactly to prevent this |
| Mandatory cloud configuration | Misrepresents the architecture — agent runs fine without |

If something isn't on this list, the implementation may treat it as in scope. Be defensive about adding to it.

---

## 4. Architecture overview — the three tiers

Different file classes have wildly different sizes, hotness, and durability requirements. A single uniform model would be wrong. Three tiers, each with its own model:

```
┌───────────────────────────────────────────────────────────────┐
│  TIER 1 — Agent state  (~1-2 MB working set)                  │
│  sessions, MEMORY.md, cron.json, SOUL.md, AGENTS.md           │
│                                                               │
│  Model:  RAM-first cache + async S3 mirror + flash snapshots  │
│  Reads:  PSRAM (always)                                       │
│  Writes: PSRAM ack → background S3 PUT (eager)                │
│          OR PSRAM + S3 round-trip (strict: memory/cron)       │
│  Boot:   GET from S3 → fall back to flash snapshot            │
│  Offline: works — writes queue in PSRAM, replay on reconnect  │
├───────────────────────────────────────────────────────────────┤
│  TIER 2 — User files  (arbitrary size)                        │
│  PDFs, images, anything uploaded via the file manager         │
│                                                               │
│  Model:  streamed from S3, never fully cached                 │
│  Reads:  ranged GETs via new file tool actions                │
│          (read_range, head, tail, info)                       │
│  Writes: direct PUT to S3, no PSRAM cache                     │
│  Boot:   nothing pre-loaded                                   │
│  Offline: tool returns "offline, retry" — agent itself        │
│           keeps working on Tier 1 cache                       │
├───────────────────────────────────────────────────────────────┤
│  TIER 3 — Config + secrets  (KB)                              │
│  provider API keys, Telegram bot token, hostname, etc.        │
│  S3 access keys themselves stay NVS-only (chicken-and-egg)    │
│                                                               │
│  Model:  NVS + S3 mirror (plain JSON, trust R2 isolation)     │
│  Boot:   read NVS; if empty (fresh device) and bucket creds   │
│          in NVS, GET sys/config.json from S3                  │
│  Offline: N/A — config loaded once at boot                    │
└───────────────────────────────────────────────────────────────┘
```

When cloud is **not configured**, behavior is identical to today: local SPIFFS only, no cloud, no banners.

## 5. Bucket layout — one bucket per device

Each device targets exactly one bucket. The bucket itself is the device's cloud identity. The mDNS hostname is decoupled from cloud identity and can change locally without affecting cloud storage.

```
my-zenclaw-livingroom/                       ← bucket = device identity
  sys/sessions/web/base.jsonl
  sys/sessions/web/log-00.jsonl
  sys/sessions/telegram-12345/base.jsonl
  sys/sessions/telegram-12345/log-00.jsonl
  sys/MEMORY.md
  sys/cron.json
  sys/SOUL.md
  sys/AGENTS.md
  sys/config.json                            ← Tier 3
  sys/.heartbeat                             ← optional safety net (see §10.4)
  files/...                                  ← Tier 2 user files
```

**Why bucket-per-device, not prefix-per-device on a shared bucket**:

- R2 free tier shares the 10 GB allowance across all buckets in an account, so multiple buckets carry no extra storage cost
- No prefix-concatenation logic needed in the storage layer
- Heartbeat conflict-detection becomes a nice-to-have (only triggers if the user explicitly types the same bucket name twice across two different wizard runs) rather than load-bearing
- Device-swap UX is dramatically simpler: "paste these bucket creds and the new device IS the old device"

---

## 6. Tier 1 — Agent state (the hot path)

### 6.1 New modules

```
agent/src/core/cloud/
  client.rs       ← exists; SigV4 + S3 client (no changes needed)
  sigv4.rs        ← exists (no changes needed)
  mod.rs          ← exists; add new exports
  replicator.rs   ← NEW — write queue + drainer thread (Tier 1 eager path)
  cache.rs        ← NEW — PSRAM-backed in-memory cache for Tier 1
  snapshots.rs    ← NEW — periodic flash snapshot writer
  boot.rs         ← NEW — boot-time restore: HEAD/GET sequence + safety layers
```

The existing `storage_tools.rs` stays as the LLM-callable interface to raw bucket operations. The new modules sit *below* it and serve the agent's session/memory/cron writers via a different (internal) interface.

### 6.2 Write paths

**Eager** (per-turn session entry):

```
agent ──► cache.rs ──► (1) update PSRAM
                  └──► (2) enqueue PUT in replicator
                  └──► (3) return Ok to agent     ← ack here

replicator drainer thread (separate)
  pop queue ──► sigv4 sign ──► HTTPS PUT to S3
            └─ on success: remove from queue
            └─ on failure: backoff (1→2→4→…→60s), retry up to 5x
            └─ exhausted: move to dead-letter, surface in /api/status
```

**Strict** (`memory_save`, cron updates, config writes):

```
agent ──► cache.rs ──► (1) update PSRAM
                  └──► (2) sigv4 sign ──► HTTPS PUT to S3
                                     └─ wait up to 5 retries (~30s worst case)
                  └──► (3) return Ok|Err to agent  ← ack here
                                                     (Err means "not persisted",
                                                      agent surfaces to user)
```

**Why hybrid**: memory and config changes are user-initiated and infrequent — paying ~500ms for a confirmed durable write is the right tradeoff because the user expects "saved" to mean saved. Per-turn session appends happen on every conversation turn — making each turn pay an S3 round-trip would visibly slow the agent. Plus session entries are naturally regenerable: drop the last turn and the user re-asks; drop a `memory_save` and the user wonders why the agent forgot.

### 6.3 Replicator queue

In-PSRAM FIFO, single drainer thread:

```rust
struct PendingWrite {
    key: String,          // S3 object key
    bytes: Vec<u8>,       // full object content (debounced)
    queued_at: Instant,
    retry_count: u8,
}
```

**Coalescing**: if a key is re-enqueued before its previous PUT starts, the new entry replaces the old. Only one pending PUT per key — preserves last-writer-wins for the same object. Critical for sessions where multiple log appends in a row should produce one PUT, not N.

**Tradeoff**: coalescing means we lose intermediate states. If you `memory_save` twice in 50ms with different content, only the second PUT happens. For sessions this is fine (the second PUT contains both appends). For arbitrary writes it's a behavior worth knowing about.

**Backpressure**: if queue depth exceeds `replicator.queue_max` (default 32), new eager writes block on a condvar until depth drops below half-cap. Hard safety valve against runaway PSRAM ↔ S3 divergence.

**Backoff**: exponential, 1s → 2s → 4s → … → 60s (`replicator.backoff_cap_secs`). After `replicator.retry_max` (default 5) attempts, the entry moves to a dead-letter queue and a banner fires in the web UI. **Surface-and-stop** — no silent forever-retry, because that masks real misconfiguration.

### 6.4 Session-specific chunking — append-log + rotating compaction

Sessions are append-only JSONL that grow continuously. Naïve "PUT the whole file on every change" is bandwidth-amplifying. We use an append-log split with **rotating log files** for compaction atomicity:

```
S3 layout:  sys/sessions/{chat_id}/base.jsonl       (snapshot, large)
            sys/sessions/{chat_id}/base.meta.json   (small — see below)
            sys/sessions/{chat_id}/log-00.jsonl     (initial log)
            sys/sessions/{chat_id}/log-01.jsonl     (after first compaction)
            sys/sessions/{chat_id}/log-NN.jsonl     (current — appends go here)

Append:     PUT log-NN (small, growing)

Compaction: log-NN > 16 KB
            1. Read base + log-NN
            2. PUT new base.jsonl = base + log-NN
            3. PUT base.meta.json = {"highest_absorbed_log": NN}
            4. Future appends go to log-(NN+1)
            (no delete of old log-NN — boot ignores it via metadata)

Boot:       1. GET base.jsonl + base.meta.json
            2. LIST sys/sessions/{chat_id}/log-* (limit pagination)
            3. For each log-XX where XX > highest_absorbed_log,
               GET and concatenate to working set
            4. Find current log-NN by highest index, write next appends there
```

**Why rotating logs**: two PUTs (`base` then `meta`) cannot be atomic across a power loss. With rotation, the failure modes are clean:

- *Crash after step 2 (new base PUT) but before step 3 (meta PUT)*: meta still points at the old log index. Boot includes the now-folded log entries via the LIST step — duplicates would happen, EXCEPT the new base file's content already includes them, so on boot we'd see `base + log-NN` appearing twice. Mitigation: include log entry IDs (already present in `SessionEntry::id()`) and dedupe on boot.
- *Crash after step 3 (meta PUT) but before any log-(NN+1) append*: clean state — boot reads new base, sees highest_absorbed=NN, ignores log-NN, finds no log-(NN+1), starts fresh log there
- *Crash mid-PUT*: S3 PUT is atomic from the client's perspective (either the object is updated or it isn't), so partial writes don't happen at the object level

Defaults: log compaction threshold 16 KB (`storage.log_compaction_bytes`, configurable), base size capped via the existing `SessionManager::compact()` (see §6.6 L2).

**Garbage collection**: old log-XX files where XX < highest_absorbed_log are dead weight. A background sweeper (runs after each successful compaction) DELETEs them. Failure to delete is non-fatal — they just consume bucket storage until next sweep.

### 6.5 Snapshot strategy (flash backup of PSRAM cache)

The flash snapshot is the **only** flash-write path in normal operation. Strategy:

- **Trigger**: every 15 min (`snapshot.interval_secs`) OR on graceful shutdown (`/api/restart` handler) OR when the replicator queue's oldest entry exceeds 5 min old (`snapshot.stale_queue_threshold_secs`) — signal that S3 isn't keeping up
- **Granularity**: full PSRAM cache snapshotted as a single tarball-ish blob to `data/.snapshot.bin` (atomic via rename-from-tmp)
- **Read on boot**: only consulted if the S3 GET in the boot flow fails entirely (network down at boot). Otherwise S3 wins
- **Wear estimate**: at 15 min cadence, ~96 writes/day. SPIFFS handles ~100K writes/cell; with wear leveling and ~2 KB writes that's order-of-magnitude better than per-turn flash writes
- **Tradeoff**: snapshotting the *whole* cache is unbounded — if a future feature pushes the working set toward 5 MB, snapshots become big writes. Mitigation: snapshot only dirty regions, deferred to v2 if it bites

### 6.6 Boot flow (full sequence with safety layers L1-L5)

```
1.  config: read from NVS
       │
       ├─ no storage block  ─► local-only mode, skip cloud restore, done
       │
       └─ storage configured ─► continue
                                    │
2.  cloud_init: instantiate S3 client with NVS creds
                                    │
3.  config_restore: GET sys/config.json
                    ├─ exists ─► merge into Config (S3 wins for non-secret fields,
                    │            NVS S3-creds preserved)
                    └─ 404    ─► fresh bucket; PUT current config as initial backup
                                    │
4.  heartbeat_check: GET sys/.heartbeat
                    ├─ different device_id, recent ts ─► WARN in /api/status,
                    │                                     proceed but flag UI banner
                    └─ ours / stale / 404 ─► PUT new heartbeat, proceed
                                    │
5.  LIST sys/sessions/  → enumerate chat_ids
    for each chat_id:
       LIST sys/sessions/{chat_id}/  → gives base.jsonl, base.meta.json, log-XX.jsonl
                    │
       L3: HEAD each key → sum content-lengths (excluding to-be-skipped logs)
                    │
       ├─ within session_max_bytes (256 KB default) ─►
       │     GET base.jsonl + GET base.meta.json
       │     parse meta → highest_absorbed_log = NN
       │     GET each log-XX where XX > NN, in order
       │     concatenate into in-memory session, dedupe by entry id
       │     note next-append target = log-(max(XX)+1) or log-(NN+1) if no logs > NN
       │
       ├─ exceeds budget                            ─►
       │     L4: tail-only ranged GET on the highest-numbered log file
       │         (last 16 KB — matches log_compaction_bytes,
       │          ≈ the most recent ~20-50 entries depending
       │          on tool-result sizes)
       │     base is dropped entirely (too large to load safely)
       │     cache populated with tail only → /api/status warning
       │     system prompt: "earlier history truncated for size"
       │
       └─ tail GET fails / parse fails              ─►
             L5: move all sys/sessions/{chat_id}/* to
                 sys/sessions/{chat_id}/.quarantine/
                 start fresh empty session (new base, new log-00)
                 web UI banner: "session {chat_id} quarantined"
                                    │
6.  memory_restore: GET sys/MEMORY.md → write to PSRAM cache
                    └─ 404 ─► proceed with empty memory (existing behavior)
                                    │
7.  cron_restore: GET sys/cron.json → load into cron registry
                                    │
8.  identity_restore: GET sys/SOUL.md, sys/AGENTS.md → cache
                                    │
9.  start replicator drainer thread, start agent loop
```

**Safety layer summary** (defense in depth):

| Layer | When | What |
|---|---|---|
| **L1** | Continuous — when log > 16 KB | Cloud-level log compaction: fold log into new base, reset log |
| **L2** | Continuous — when base would exceed `session_max_bytes` | Trigger existing `SessionManager::compact()` — drop old messages, keep summary |
| **L3** | At boot, before GET | HEAD each session object; gate GET on size budget |
| **L4** | At boot, when L3 trips | Tail-only ranged GET (last K bytes); agent loses old context but boots |
| **L5** | At boot, when L4 also fails | Quarantine the offending object; start fresh empty session; web UI banner |

L1 and L2 are prevention; L3, L4, L5 are the catch nets. All five enforced by default. Configurable budget via `storage.session_max_bytes` (default 256 KB).

L4 truncations surface in the agent's system prompt so it can ask the user about lost context if relevant. L5 fires automatically (alternative is the device unable to boot the chat at all, which is worse than fresh-start with a recoverable banner).

### 6.7 Failure modes (Tier 1)

| Failure | Behavior |
|---|---|
| Network drop mid-eager-write | Write sits in queue, retries with backoff. Agent unaffected. |
| Network drop mid-strict-write | After 5 retries (~30s), tool returns error to agent. Agent tells user "not saved". |
| S3 returns 4xx (auth/perm error) | Same as exhausted retries → dead-letter → surface in `/api/status` and web UI banner. |
| Power loss with eager queue non-empty | Recent session entries lost (acceptable — user re-asks). Memory/cron/config never lost (strict path). |
| S3 unreachable at boot | Fall back to flash snapshot. If snapshot also missing/corrupt → empty cache, log warning, agent runs as if first boot. |
| Bucket exists from a different device | Heartbeat warning surfaced, sync proceeds (user must explicitly "take over" via web UI to suppress). |

---

## 7. Tier 2 — User files (chunked S3 reads)

### 7.1 `file` tool extensions

| Action | Args | Returns | Notes |
|---|---|---|---|
| `read` *(capped)* | `path` | content (≤32 KB) | If file is larger than 32 KB, returns an error with hint: *"file is N bytes; use read_range, head, or tail"* |
| `read_range` *(new)* | `path`, `offset`, `length` (default 16 KB, max 32 KB) | content slice | Tier 1 paths: served from PSRAM. Tier 2 paths: ranged GET from S3. |
| `head` *(new)* | `path`, `length` (default 4 KB, max 32 KB) | first N bytes | Convenience wrapper over `read_range(0, length)`. |
| `tail` *(new)* | `path`, `length` (default 4 KB, max 32 KB) | last N bytes | Tier 2: requires `info` first to get size; cached for 60s per path. |
| `info` *(new)* | `path` | `{size, etag, last_modified, tier}` | Tier classification surfaced so the LLM knows whether reads will be cheap (Tier 1 PSRAM) or network-bound (Tier 2 S3). |

The existing `write`, `edit`, `delete`, `list_dir` actions get tier-aware routing internally but their LLM-visible shape is unchanged.

### 7.2 `storage` tool — unchanged

Stays as the LLM's escape hatch for raw bucket operations: cross-device queries ("did my office device save anything to its bucket today?"), debugging, one-off scripts. The new tier-aware routing in `file` is the **default path**; `storage` is for advanced use.

The system prompt's tooling section will be updated to disambiguate: *"file = your normal interface; storage = raw bucket access for cross-device or debugging."*

### 7.3 Tier 2 writes

```
agent / web UI ──► sigv4 sign ──► HTTPS PUT to S3
                              └─ no PSRAM cache, no replicator
                              └─ for files > 5 MB, multipart upload
                                (chunk into 5 MB parts, upload sequentially)
```

No queue, no coalescing — direct path to S3. Failures surface to the caller immediately (the file tool returns an error; web UI surfaces it).

---

## 8. Tier 3 — Config + secrets

### 8.1 What's stored where

| Where | What |
|---|---|
| **NVS** | S3 access key ID, S3 secret access key, S3 endpoint, S3 bucket name, S3 region |
| **NVS + S3 (`sys/config.json`)** | Everything else in `Config`: provider API keys, Telegram bot token, hostname, agent_name, heartbeat config, channels config, etc. |

S3 creds are **never** PUT to S3 — chicken-and-egg. They live only in NVS, populated by the wizard.

**Plain JSON in S3** (no device-side encryption). Trust R2's private-bucket isolation. If the user wants stronger security, they should rotate S3 keys on a schedule and use bucket-scoped keys (read/write to one bucket only) — neither of which we can enforce, but both simpler than passphrase management.

### 8.2 Device-swap flow

The wizard adds a forking question: *"Are you setting up a new device, or recovering from a previous one?"*

```
WIZARD FLOW (existing → with cloud persistence)
─────────────────────────────────────────────────
1. Pick board (DevKitC / Guition P4)              [existing]
2. Detect chip via esptool-js                     [existing]

3. Setup mode? ──┬─ Fresh setup (default)
                 │   └─ continue to step 4 (existing flow)
                 │
                 └─ Restore from previous device
                       │
                       ├─ Step 3a: Bucket credentials
                       │   • Endpoint, bucket name, access key, secret
                       │   • [Test connection] button → /api/cloud/test
                       │   • Test passes → enable next button
                       │
                       ├─ Step 3b: Recovered config preview
                       │   • Wizard fetches sys/config.json from bucket
                       │   • If sys/config.json is missing (bucket is empty
                       │     or never had a previous device): wizard surfaces
                       │     "this bucket has no recoverable data" and
                       │     offers to switch to fresh setup mode
                       │   • Otherwise shows recovered hostname, provider, etc.
                       │   • Editable — defaults to recovered hostname
                       │     (decoupled from bucket; can override)
                       │   • [Confirm] proceeds
                       │
                       └─ skip steps 4 and 5 — use recovered values
                                       │
                                       ▼
4. Device name + WiFi creds (skipped on restore)  [existing]
5. LLM provider + API key (skipped on restore)    [existing]

6. Cloud backup (NEW step — appears in fresh flow,
   pre-filled in restore flow):
   • [Recommended] Configure cloud backup
       - Bucket creds (or pre-filled if restoring)
       - [Test connection] before allowing flash
   • [Skip] I'll configure later
       - Confirms with: "without backup, conversations and
         memory will be lost if this device fails"
       - Sets a flag the dashboard banner reads

7. Flash firmware + write NVS                      [existing,
                                                    NVS now includes
                                                    storage block]

8. Wait for device on mDNS, push /api/config       [existing,
                                                    skipped on restore]
```

---

## 9. HTTP API changes

| Endpoint | Change |
|---|---|
| `/api/status` | Existing `cloud_storage` block extended (see §10.1). Existing 60s LIST cache stays. |
| `/api/cloud/test` *(new)* | POST `{endpoint, bucket, access_key_id, secret_access_key, region}` — drives a real round-trip (`PUT sys/.test-{uuid}` → `GET` it back → `DELETE`). Returns `{ok: true}` or `{ok: false, error: "...", stage: ...}` where `stage` is one of: `endpoint_unreachable`, `auth_failed`, `bucket_not_found`, `permission_denied` (no PUT/GET/DELETE perm), `roundtrip_corrupt` (GET returned different bytes than PUT), `other`. Used by the wizard "Test connection" button before commit. |
| `/api/cloud/takeover` *(new)* | POST — explicit user action to suppress a heartbeat-conflict warning (writes a fresh heartbeat with this device's id). Surfaced as a button on the warning banner. |
| `/api/files` | When storage configured and the requested path is under `files/`, routes to S3 (PUT/GET/DELETE). When unconfigured, behaves as today (local FS). Web UI cloud browser uses this endpoint unchanged — routing happens server-side. |
| `/api/config` | When storage configured, every successful POST also triggers a strict-path PUT to `sys/config.json` before the existing reboot. If the strict PUT fails, the reboot is aborted and the API returns 503 with the failure detail. |

---

## 10. Observability

### 10.1 `/api/status.cloud_storage` shape

```json
{
  "enabled": true,
  "bucket": "my-zenclaw-livingroom",
  "endpoint": "https://abc123.r2.cloudflarestorage.com",
  "region": "auto",

  "sync": {
    "queue_depth": 0,
    "queue_max": 32,
    "last_sync_ts": 1714738293,
    "last_sync_age_secs": 12,
    "dead_letter_count": 0
  },

  "snapshot": {
    "last_snapshot_ts": 1714737900,
    "next_snapshot_ts": 1714738800,
    "snapshot_size_bytes": 124680
  },

  "boot_warnings": [
    {
      "kind": "truncated",
      "chat_id": "telegram-789",
      "original_size": 421000,
      "kept_size": 16384,
      "at": 1714730000
    }
  ],

  "failures": [
    {
      "key": "sys/sessions/web/log-00.jsonl",
      "retry_count": 5,
      "last_error_at": 1714738100,
      "last_error_msg": "S3 503 SlowDown"
    }
  ],

  "heartbeat": {
    "ours": true,
    "conflict_with": null
  },

  "usage": {
    "objects_total": 27,
    "bytes_total": 2148392,
    "last_listed_at": 1714738200
  }
}
```

### 10.2 Logging categories

- `cloud::replicator` — queue ops, retry attempts, dead-letter promotions
- `cloud::boot` — restore sequence, safety-layer triggers (L3/L4/L5 fires)
- `cloud::snapshot` — snapshot writes, stale-queue triggers

Volume is bounded — replicator writes one line per PUT (success or fail), not per byte. At 1 turn per minute, ~1440 lines/day — well within `/ws/logs` capacity.

### 10.3 Web UI surface

A new **Cloud Status** card on the Dashboard:

```
┌────────────────────────────────────────────────────────┐
│  ☁  Cloud Status                              [Details]│
│                                                        │
│  Bucket: my-zenclaw-livingroom    (R2)                 │
│  Synced: 12 seconds ago                                │
│  Queue:  0 pending                                     │
│  Usage:  2.1 MB / 10 GB free tier                      │
│                                                        │
│  Last snapshot: 8 minutes ago                          │
└────────────────────────────────────────────────────────┘
```

Card turns yellow when sync is stale (>5 min) or boot warnings exist. Red when dead-letter has entries or heartbeat conflict detected.

### 10.4 Dashboard banners

**When cloud is unconfigured** (non-dismissable until configured):

```
┌────────────────────────────────────────────────────────────────┐
│  ⚠  No cloud backup configured                                 │
│                                                                │
│  Your conversations, memory, and configuration are stored      │
│  only on this device's flash. Power loss, firmware updates,    │
│  or hardware failure will erase them.                          │
│                                                                │
│  Cloudflare R2's 10 GB free tier is enough for years of        │
│  conversation history.                                         │
│                                                                │
│  [Configure cloud backup →]                                    │
└────────────────────────────────────────────────────────────────┘
```

**When cloud IS configured but something is wrong**:

- *Boot warnings present* (truncated/quarantined sessions): yellow banner with link to per-session detail in `/api/status`
- *Heartbeat conflict detected*: red banner with `[Take over]` button → `/api/cloud/takeover`
- *Dead-letter queue non-empty*: red banner listing failed keys, with `[Retry now]` and `[View details]` buttons

---

## 11. Config schema

```rust
// agent/src/config.rs — StorageConfig extensions
pub struct StorageConfig {
    pub path: Option<String>,                  // existing — local data dir override

    // S3 creds (existing)
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    #[serde(default = "default_storage_region")]
    pub region: String,

    // NEW — cloud persistence behavior
    #[serde(default = "default_session_max_bytes")]
    pub session_max_bytes: usize,              // default 256_000 (256 KB)
    #[serde(default = "default_log_compaction_bytes")]
    pub log_compaction_bytes: usize,           // default 16_384 (16 KB)

    #[serde(default)]
    pub replicator: ReplicatorConfig,
    #[serde(default)]
    pub snapshot: SnapshotConfig,
}

#[derive(Default)]
pub struct ReplicatorConfig {
    pub queue_max: u32,                  // default 32 — backpressure cap
    pub retry_max: u8,                   // default 5
    pub backoff_cap_secs: u32,           // default 60
}

#[derive(Default)]
pub struct SnapshotConfig {
    pub interval_secs: u32,              // default 900 (15 min)
    pub stale_queue_threshold_secs: u32, // default 300 (5 min)
}

// Cloud-persistence is "active" when bucket + keys are all present.
// Helper method on Config:
//   fn is_cloud_enabled(&self) -> bool { self.storage.bucket.is_some() && ... }
```

Defaults are enough for normal use. Power users tune via the Config page. The wizard never exposes the replicator/snapshot knobs; only bucket + creds.

---

## 12. Migration: existing devices

After this feature ships, when a user enables cloud for the first time on an existing device with data already in flash:

```
1. POST /api/config with new storage block triggers reboot (existing behavior)
2. Boot flow detects: storage.bucket is set + bucket is empty (HEAD on sys/.heartbeat returns 404)
3. Initial-backup phase:
   - Read all of data/sessions/*, data/MEMORY.md, data/cron.json,
     data/SOUL.md, data/AGENTS.md from local flash
   - PUT each to S3 under sys/* (single batch, sequential, blocks boot)
   - PUT current Config to sys/config.json
   - PUT initial heartbeat
4. Proceed with normal boot flow — now everything is durably in S3
5. Subsequent writes follow the normal eager/strict tiered routing
```

The initial-backup phase shows a banner in the web UI ("Migrating to cloud — N MB to upload"). Migration is one-way; once done, S3 is the source of truth and local flash becomes a snapshot fallback.

If migration fails partway (network drop), the next boot retries — already-uploaded files get HEAD'd and skipped. The boot is **blocked** during initial migration (typical ~5-15s for ~500 KB) — chosen over background migration because the latter has a window where some writes go to S3 and others don't yet.

If migration fails for a non-recoverable reason (auth failure: user typed wrong S3 creds; bucket-not-found: typo): after `replicator.retry_max` retries, the device:

1. Falls back to **local-only mode** (no cloud sync, agent works as it did before configuration)
2. Surfaces a non-dismissable red banner in the web UI: *"Cloud migration failed: {error}. Fix credentials in Config and reboot to retry."*
3. Logs the failure to `/ws/logs` with the exact stage/error

The user is never permanently locked out of their device by a misconfigured cloud setup — local-only is always the safe fallback.

---

## 13. Testing strategy

### 13.1 Unit (host)

- `cloud::replicator::tests` — coalescing logic, backpressure, retry/backoff, dead-letter promotion. No real S3; injects a fake `S3Client` trait impl
- `cloud::cache::tests` — PSRAM-cache invariants under concurrent reads/writes
- `cloud::boot::tests` — safety-layer trip points (L3 size gate, L4 tail logic, L5 quarantine), mocked S3 returning various sizes/errors

### 13.2 Integration (real S3 against test bucket)

- `cloud::client::tests` — already exists; covers the SigV4 + S3 client surface
- New `cloud::e2e::tests` — full write-queue + read-restore cycle against a test R2 bucket. Gated behind a `--features integration-test` flag; CI uses a MinIO docker container

### 13.3 On-device smoke tests

- Run the existing chat smoke against a DevKitC with cloud configured; verify session entries appear in S3
- Hard-power the DevKitC mid-conversation; verify next boot restores correctly via S3 (and logs whether snapshot fallback fired)
- Configure a fake R2 endpoint that returns 503; verify dead-letter surfaces in `/api/status` and the agent loop continues working
- Migration: take an existing on-device session, configure cloud, verify the migration banner fires and S3 ends up populated

### 13.4 Explicitly NOT tested in v1

- Multi-region replication (one bucket, one region)
- Concurrent writes from two devices to the same bucket (the heartbeat warning is the test; we don't try to make this work)
- Encryption-at-rest (delegated to R2's bucket-level encryption)

---

## 14. Implementation order (preview)

The detailed implementation plan comes next from the writing-plans skill. Rough sequence so the slice sizes are sane and each PR is independently mergeable:

1. **`cloud::cache` + `cloud::replicator`** (foundation) — PSRAM cache + write queue with coalescing. Behind a flag, no boot integration yet
2. **Tier 1 write hooks** — patch `SessionManager::append`, `memory_tools::write`, `cron::save`, `Config::save` to route through the cache when cloud is enabled
3. **`cloud::boot`** — restore sequence with safety layers L3/L4/L5
4. **`cloud::snapshots`** — flash snapshot writer + boot-time fallback path
5. **`/api/status.cloud_storage`** extensions + `/api/cloud/test` + `/api/cloud/takeover`
6. **Tier 2 (`file` tool extensions)** — `read_range`, `head`, `tail`, `info`. Tier-aware routing in existing `read`/`write`/`edit`
7. **`/api/files` transparent routing** for `files/` paths
8. **Web UI**: provisioning wizard step, dashboard banner + Cloud Status card, recovery-mode wizard branch
9. **Migration path**: initial-backup phase for existing devices
10. **Logging + observability + on-device smoke tests**
11. **Update README Roadmap** — move item #1 from "shipped when…" to "shipped"

Steps 1–4 ship the engine without UI; user could opt in via `/api/config` POST and have a working system. Steps 5–8 surface it. Step 9 is the device-already-in-the-wild migration.

---

## 15. Open questions / future considerations

- **Snapshot granularity**: full PSRAM cache is simple but unbounded. If working set grows toward several MB, switch to dirty-region snapshotting. Defer until it bites.
- **Multi-region buckets**: not needed for v1. If a user runs devices in geographically distant locations and wants the closest R2 region, add `region` per-bucket-target — but only after multi-bucket lands (currently out of scope).
- **R2 LIST cost**: the boot flow's per-chat_id LIST in step 5 could be batched into a single `LIST sys/sessions/` and grouped client-side if cost becomes a concern. Defer.
- **Snapshot encryption**: the local `data/.snapshot.bin` contains the same plaintext as S3. If a user wants encryption, it'd live at this layer. Defer with the rest of the encryption story.

---

## Appendix — decision log (from brainstorming session 2026-05-02)

| # | Question | Decision | Rationale |
|---|---|---|---|
| 1 | Scope | Everything in `data/` + config recovery for device swap | "Fully recover whole work and configuration" |
| 2 | Storage architecture | Tiered: T1 RAM-first, T2 streamed, T3 NVS+S3 | Single architecture wrong; PSRAM blowup for large files; offline must keep working |
| 3 | Session chunking | Append-log + periodic compaction | Same approach as Kafka/WAL; bounded R2 cost; fast boot |
| 4 | Boot safety layers | All 5 (L1-L5); 256 KB session_max_bytes default; configurable | Defense in depth; runaway logs must not brick devices |
| 5 | Bucket layout | Bucket-per-device | R2 free tier shares storage; simpler code; bucket = identity |
| 6 | Write queue semantics | Hybrid (strict for memory/cron/config, eager for sessions); surface-and-stop on dead-letter | Latency for sessions; durability for memory; user must know about failures |
| 7 | UX intensity | Wizard step + non-dismissable banner if skipped | Strong without being a hard wall; dashboard is the second touchpoint |
| 8 | Config secrets | Plain JSON in S3 (no device-side encryption) | Trust R2 isolation; passphrase mgmt is more friction than security gain for target audience |
