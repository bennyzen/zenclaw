# Stats transport model — WS-primary, GET-fallback

**Date:** 2026-05-03
**Status:** Approved, replaces the previous concurrent WS+GET stats model
**Scope:** Reshape how the web UI receives device status

## Problem

The web UI runs **two redundant stats streams concurrently**:

- `/ws/stats` WebSocket pushes a lean payload (memory / temperature / wifi
  RSSI / storage / uptime) every 10 s. (Originally 5 s on ESP32 / 3 s on
  desktop; bumped to a symmetric 10 s once the unified payload landed —
  the slow-changing fields don't justify the tighter cadence.)
- `/api/status` HTTP poll fetches the **full** payload (above + agent
  identity, board, platform, provider, model, channels, cloud_storage,
  network info, USB) every 15 s.

Both run all the time. The footer reads from a merged store. This produced
three bugs in close succession:

1. `provider`/`model` were only set once at connect time and never refreshed
   — switching the active model required a full page reload.
2. A first attempt to fix this by making the merge include `provider`/`model`
   from `/api/status` worked when polling won the race, but the `/ws/stats`
   payload doesn't carry those fields. The next WS frame overwrote them
   with `null`. Footer flickered every 10 s.
3. Adding a `?? prevValue` fallback masked a real asymmetry between
   `/api/status` and `/ws/stats`. The asymmetry existed because the two
   endpoints were designed for different roles, not because of platform
   differences.

The underlying issue is that two transports for the same data, with
different shapes, force the consumer to merge by hand. Hand-merging across
two non-equal payloads is fragile.

## Goals

- One **shape** for device status everywhere it appears.
- One **transport** active at a time.
- Web is a dumb mirror: it never displays anything the device hasn't
  confirmed. No optimistic local updates after a config switch.
- Switching the active model (or any other config-derived field) is
  reflected in the UI as soon as the device pushes its post-reboot state.

## Non-goals

- Reducing the WS push interval below 10 s. The fields the WS carries
  (free heap, RSSI, temperature, storage, uptime) all change slowly;
  10 s is the sweet spot between liveness and per-device overhead.
- Adding "delta only" or change-detection on the device. Pushing the full
  payload at 10 s is well within budget given the cloud-status block is
  already 60 s-cached server-side.
- Introducing per-field subscription channels.

## Design

### One payload, two transports

The device exposes the *same JSON shape* on two transports:

- **`GET /api/status`** — single-shot request/response. Used once on initial
  connect and as a fallback poll when the WebSocket is down.
- **`WS /ws/stats`** — server-push. Sends the same payload every 10 s while
  the connection is open.

Both transports are served by a single `build_status_payload()` function
on the device side. There is no second JSON shape to maintain. ESP32 and
desktop builds each have their own builder (one of the platform's
responsibilities is to fill in fields the other can't), but on each
platform, GET and WS share the builder.

### Web composable owns the transport state machine

`web/app/composables/useConnection.ts` runs **at most one** stats transport
at a time:

```
                    networkConnected → true (after connectNetwork)
                              │
                              ▼
                     ┌─────────────────┐
                     │ Try WS open     │
                     └────────┬────────┘
                              │
              ┌───────────────┴────────────────┐
              ▼                                ▼
        WS opens                          WS fails
              │                                │
              ▼                                ▼
     ┌─────────────────┐              ┌─────────────────┐
     │ stopStatsPoll() │              │ startStatsPoll  │
     │ ws.onmessage →  │              │ (15 s GET poll) │
     │   setStatus(…)  │              │ + retry WS in 5 s│
     └────────┬────────┘              └────────┬────────┘
              │                                │
        WS closes                       WS retry succeeds
              │                                │
              ▼                                ▼
     ┌─────────────────┐              ┌─────────────────┐
     │ startStatsPoll  │              │ stopStatsPoll   │
     │ + retry WS      │              │ (back to WS)    │
     └─────────────────┘              └─────────────────┘
```

When the WS is open, polling is **off**. When the WS closes (device
reboot, network blip, etc.), polling kicks in as a fallback and the WS
is retried every 5 s. When the WS reopens, polling stops. Three
consecutive poll failures still trip the existing
"disconnect + reconnect" path.

Every payload — whether from WS or GET — feeds the same `setStatus(raw)`
function: `state.lastStatus = mapStatus(raw)`. **Full replace, not merge.**
Because both transports carry the same shape, partial-merge is no longer
needed.

### Web is a dumb mirror

When the user clicks **Switch & Reboot** in the model fast-dial, the
client:

1. PUTs `/api/config` with `providers.default` set to the chosen slug.
2. Shows the success banner ("Device is rebooting…").
3. The device reboots (~12 s on ESP32). The WebSocket closes.
4. Polling kicks in. Polls fail until the device is reachable again.
5. The WS retry succeeds. The next push includes the new `provider`/
   `model`. `setStatus` replaces `state.lastStatus`. Footer updates.

The web UI never **assumes** the new state is active. If the device
fails to apply the change (rejects the JSON, fails to write NVS, panics
during reboot), the UI reflects the actual post-reboot device state, not
a guessed one.

### Removal of incidental fallbacks

- `connectNetwork` previously fetched `/api/config` to fill in the model
  when `/api/status` didn't include one. With the unified payload this
  fallback is removed.
- `mergeMetrics`/`mergeStatus` (the partial-update functions) are
  collapsed into `setStatus` (full replace).
- The `?? prevValue` defensive fallback for missing fields is removed —
  every payload is full and every field is present (`null` where not
  applicable).

## What "ALL values" means

The WS and GET payloads carry the same fields the `DeviceStatus`
interface in `web/types/connection.ts` declares — i.e. everything the UI
displays:

- `agent_name`, `version`, `built`, `board`, `platform`
- `memory`, `temperature_c`, `wifi`, `network`, `storage`, `usb`
- `channels.telegram.{configured, enabled, has_token}`
- `cloud_storage` (60-s-cached server-side)
- `provider`, `model`
- `uptime_s`

**Secrets are never shipped over the WS or in `/api/status`.** API keys,
secret access keys, telegram bot tokens — these live in `/api/config`
and are gated behind the explicit GET. The status payload only carries
*derived display flags* like `channels.telegram.has_token` (boolean
"is a token configured", not the token itself).

## Rollback

If the WS-primary model causes problems, the previous concurrent
WS+GET model is one revert away. The on-disk config and the agent
endpoints are unchanged in shape; only the consumer's transport
selection logic changes.

## Implementation notes

- Server: extract `build_status_payload(...)` near `cloud_status_block`
  in `agent/src/main.rs`. Both `/api/status` and `/ws/stats` call it.
  Same for `agent/src/desktop/server.rs`.
- Client: replace `mergeMetrics` and `mergeStatus` with `setStatus`.
  Replace `startStatsStream` with `ensureStatsTransport` that owns the
  WS lifecycle. Polling is gated by WS state.
- Leave the existing 3-poll-fail → reconnect path untouched. It still
  applies during the polling fallback window.

## Out of scope

- Per-field subscriptions / change events on the device.
- Reducing WS frequency adaptively (e.g. slow when no metrics changed).
- Splitting `/api/status` into a config-only and a metrics-only endpoint.
