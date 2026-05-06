# Multi-Conversation Web UI — End-to-End Playbook

**Companion to:** `docs/superpowers/specs/2026-05-05-multi-conversations-design.md` and `docs/superpowers/plans/2026-05-05-multi-conversations.md`.

**Run on:** devkitc AND guition-p4 (separately) before declaring v1 shipped.

## Prerequisites

- Device flashed with the `feat/multi-conversations` branch firmware (use the web wizard at `https://bennyzen.github.io/zenclaw/provision`, not `just flash`).
- Device online; mDNS name resolved (`zenclaw-<your-name>.local`).
- `jq` and `curl` installed locally.
- For Playwright steps: dev server (`cd web && npm run dev`) on `http://localhost:3000` AND a Chromium-based browser.

```bash
HOST=zenclaw-<your-name>.local
```

## Part A — REST smoke (curl)

Verifies the backend independently of any browser CORS plumbing.

### A1. Create + list

```bash
ID=$(curl -sf -X POST http://$HOST/api/sessions | jq -r .chatId)
echo "Created: $ID"

curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\")"
```

**Expected:** `chatId` starts with `chat-`; the listed object has `kind: "web"`, `title: "New chat"`, `titleSource: "default"`, both timestamps populated, empty `lastMessagePreview`, `version: 1`.

### A2. Send a message → bump_activity + LLM title

```bash
curl -sf -X POST "http://$HOST/api/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"message\":\"How do I propagate tomatoes from cuttings?\",\"chat_id\":\"$ID\"}" \
  | jq -r .reply | head -3

# Verify the row was bumped (lastActivityMs > 0, preview populated):
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\")"

# Wait ~5s for the title-gen background task to complete:
sleep 5

# Verify titleSource transitioned Default → Llm:
curl -sf "http://$HOST/api/sessions" | jq ".[] | select(.chatId==\"$ID\") | {title, titleSource}"
```

**Expected:** after the chat completes, `lastMessagePreview` reflects the assistant's reply (truncated to 120 chars). After ~5s, `titleSource: "llm"` and `title` is a 6-words-or-fewer summary (e.g., `"Tomato propagation"`). If `titleSource` stays `"firstMessage"` or `"default"`, the LLM call may have failed silently — check device logs (`espflash monitor` or `/api/status` dead-letter section).

### A3. Rename

```bash
curl -sf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' \
  -d '{"title":"smoke-test"}' \
  | jq '{title, titleSource}'
```

**Expected:** `{"title": "smoke-test", "titleSource": "user"}`.

### A4. Validation errors

```bash
# Empty title → 400
curl -isf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' \
  -d '{"title":""}' | head -1

# Whitespace-only → 400
curl -isf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' \
  -d '{"title":"   "}' | head -1

# 81-char title → 400
LONG=$(printf 'a%.0s' {1..81})
curl -isf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' \
  -d "{\"title\":\"$LONG\"}" | head -1

# Non-existent chat → 404
curl -isf -X PATCH "http://$HOST/api/sessions/does-not-exist" \
  -H 'Content-Type: application/json' \
  -d '{"title":"x"}' | head -1
```

**Expected:** first three return `HTTP/1.1 400`, last returns `HTTP/1.1 404`. (Exact phrase varies by reason text; the status line matters.)

### A5. Delete + cloud cleanup

```bash
curl -sf -X DELETE "http://$HOST/api/sessions/$ID" -w "%{http_code}\n"

# Verify gone from the list:
curl -sf "http://$HOST/api/sessions" | jq "[.[] | select(.chatId==\"$ID\")] | length"

# Verify cloud cleanup (only if cloud is enabled on this device):
curl -sf "http://$HOST/api/cloud/files?prefix=sys/sessions/$ID/" | jq '.files | length'
```

**Expected:** delete returns `204`. List length is `0`. Cloud-files length is `0` (if cloud enabled).

### A6. Idempotent delete

```bash
curl -sf -X DELETE "http://$HOST/api/sessions/$ID" -w "%{http_code}\n"
```

**Expected:** `204`. Re-deleting a missing chat is a no-op, not an error.

### A7. Wildcard syntax verification

This proves T14's `uri_match_wildcard: true` fix works.

```bash
# Create a fresh chat:
ID=$(curl -sf -X POST http://$HOST/api/sessions | jq -r .chatId)

# Path-param PATCH must work (not 404):
curl -isf -X PATCH "http://$HOST/api/sessions/$ID" \
  -H 'Content-Type: application/json' \
  -d '{"title":"wildcard works"}' | head -1

# Empty chat_id (URI is exactly /api/sessions/) must return 400, not crash:
curl -isf -X DELETE "http://$HOST/api/sessions/" | head -1

# Cleanup:
curl -sf -X DELETE "http://$HOST/api/sessions/$ID"
```

**Expected:** PATCH returns `HTTP/1.1 200`, the empty-chat_id DELETE returns `HTTP/1.1 400`.

### A8. CORS preflight

This proves T14's CORS fixes work for browsers.

```bash
# Preflight for PATCH on a wildcard path:
curl -isf -X OPTIONS "http://$HOST/api/sessions/foo" \
  -H 'Origin: http://localhost:3000' \
  -H 'Access-Control-Request-Method: PATCH' \
  -H 'Access-Control-Request-Headers: content-type' \
  | head -10
```

**Expected:** status `204` with response headers including:
- `Access-Control-Allow-Origin: *`
- `Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS` (PATCH must be present)

If PATCH is missing or the response is `404`, T14's CORS fix didn't take effect — reflash from the latest wizard firmware.

## Part B — Browser end-to-end (Playwright MCP)

The browser flow exercises the full stack including the Vue UI.

### Setup

1. Start the Nuxt dev server: `cd web && npm run dev` (default port 3000).
2. Open Chromium-based browser (Chrome/Edge — required for Web Serial in provisioning, optional for this playbook).
3. Open `http://localhost:3000`.
4. In the connection input at the top, enter your device hostname (e.g., `zenclaw-tomato`) and click Connect. Verify the green "connected" indicator.

### Steps

1. **Sidebar appears.** Navigate to `http://localhost:3000/chat`. Verify the left-side sidebar appears with "New chat" button and search input.
2. **New chat creates and routes.** Click "New chat". URL should change to `/chat/chat-<epoch_ms>`. A new row appears at the top of the sidebar with title "New chat".
3. **Send a message.** Type `ping` in the compose box, submit. Verify the assistant reply renders.
4. **Sidebar bumps.** The row's `lastActivityMs` should update (visible as the row staying at the top with a fresh "now" timestamp). Preview should show the assistant's reply text (truncated).
5. **LLM title appears.** Wait ~5–10 seconds. The row's title should change from "New chat" to an LLM-summarized title.
6. **Search filter.** Type a substring of the title in the search box. Verify the row filters in/out.
7. **Rename in place.** Click the row's kebab menu → "Rename". The title becomes editable. Type "Custom title" and press Enter. Verify the row updates immediately.
8. **Persistence across reload.** Refresh the page (`Ctrl+R`). Reconnect if needed. Verify the renamed row's title is still "Custom title".
9. **Delete with confirm.** Click the row's kebab → "Delete". A modal appears asking for confirmation. Click "Delete". Verify the row disappears AND the URL navigates back to `/chat`.
10. **Empty state.** Delete all sessions (or visit a fresh device). The sidebar shows "No conversations yet — click 'New chat' to start."
11. **Disconnect handling.** Disconnect the device (unplug Ethernet on P4, or kill the WiFi on S3). The sidebar should empty (next refresh fails gracefully). Reconnect; the sidebar should re-populate from the cache.

### Failure modes to spot

- **Sidebar empty in cloud mode after fresh boot.** This would mean T10's materialization step (`uri_match_wildcard` was the runtime concern; materialization writes `<id>.{jsonl,meta.json}` to local fs) didn't run. Check device logs for `boot_restore: materialize` messages.
- **Rename fails silently.** Open browser devtools → Network. The PATCH should return 200. If 4xx/5xx, check the toast (T16 `useToast` integration).
- **LLM title never appears.** The post-turn task may have failed. Check `/api/status` for dead-letter entries; check device logs for `title_gen for ...: LLM call failed` warnings.
- **Layout overflow when ConnectionBanner is visible.** The chat layout's `h-[calc(100vh-8rem)]` doesn't account for the banner's extra height. Reconnect to dismiss the banner; document for follow-up if it interferes.
- **Sidebar updates on every assistant_text chunk.** During a long streaming reply, the sidebar may flicker as the preview updates per chunk (T17 quality-review I1). If visually jarring, gate the `bumpLocal` call on `evt.final` in `pages/chat/[id].vue`.

## Part C — Boot-restore round-trip (cloud-enabled devices only)

Proves T10's materialization step works.

1. Create a chat via the UI (Part B step 2). Send a message. Wait for title to populate.
2. Note the `chatId` from the URL.
3. Reboot the device:
   ```bash
   curl -sf -X POST "http://$HOST/api/restart"
   ```
4. Wait ~15 seconds for the device to come back online.
5. Reconnect via the web UI. Open `/chat`.
6. **Expected:** the chat from step 1 appears in the sidebar with the same title and a populated preview. Click into it; history loads correctly.

If the chat doesn't appear, the cache→local-fs materialization didn't fire. Verify in device logs (`espflash monitor`) for the `boot_restore: materialize` warnings. If those are absent, check that the BootConfig at `agent/src/main.rs:bootstrap_cloud` includes `sessions_dir: Some(format!("{}/sessions", data_dir))`.

## Sign-off checklist

Before merging the feature branch:

- [ ] Part A all steps pass on **devkitc** (S3-DevKitC).
- [ ] Part A all steps pass on **guition-p4**.
- [ ] Part B all steps pass against **devkitc** via the dev server.
- [ ] Part B all steps pass against **guition-p4** via the dev server.
- [ ] Part C boot-restore round-trip passes on at least one cloud-enabled device.
- [ ] No regressions in pre-existing chat behavior (single-chat sending, history loading, MDC rendering, scroll anchoring).
- [ ] No new entries in `/api/status` dead-letter that didn't exist before.
- [ ] CORS preflight (Part A8) returns `Access-Control-Allow-Methods` containing PATCH.

If any check fails, file a bug rather than declaring v1 shipped.
