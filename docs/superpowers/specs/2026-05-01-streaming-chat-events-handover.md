# Streaming chat events — handover

**Date:** 2026-05-01
**Branch:** `feat/streaming-chat-events`
**Origin spec:** `2026-05-01-streaming-chat-events-design.md` (same dir)

This handover captures the state of the streaming-chat-events feature for resumption in a fresh session. The desktop path is verified end-to-end. The ESP32 path is partially working — see "Outstanding issue" below.

## TL;DR

- **Desktop streaming works.** Real `thinking_started → tool_call_started → tool_call_finished → assistant_text → done` arrives over `/ws/chat`. Verified with a Python WS client against `localhost:8080`.
- **ESP32 streaming is partially working.** WS frames now parse correctly and `thinking_started` is delivered to the browser, but the LLM HTTPS call from the device hangs (>2min, no response). REST `/api/chat` also hangs after the latest flash.
- **z.ai is fine.** The same provider returns in 5.8s from the desktop binary on `localhost:8080`.
- The hang is post-flash and device-specific. Top hypothesis is the parallel Telegram-unification commits (`432516c` + `2683b16`) on this branch — they don't touch `esp32/runner.rs` but they construct a permanent `Arc<dyn HttpClient>` at boot and changed `agent_thread`'s signature.

## What's done & verified

### Desktop
- Full event flow over `/ws/chat`: typed `ChatEvent` enum at `agent/src/core/chat_events.rs`, threaded through `Gateway::chat_with_events` → `agent_loop::run_loop` → `execute_tool_calls`.
- `/api/chat/history` returns `{events: [...]}` synthesized from the JSONL session branch.
- Web UI (`web/app/pages/chat.vue`) reduces events into a timeline with inline tool disclosures, "thinking" pulse, and a Cancel button.
- New binary is running on `localhost:8080` (replaced the user's stale 11:06 process).
- Smoke test: `python3 -c '...websockets...'` on `ws://localhost:8080/ws/chat` shows full event sequence and a sensible reply.

### ESP32
- Build clean: `just build devkitc` succeeds (release profile).
- `/api/chat/history` returns `{events: [...]}` (verified — the *new* shape is on the device).
- WS handler correctly parses inbound `user_message` frames after the trailing-zero trim fix.
- `thinking_started` event reaches the browser.
- Device firmware on disk includes all new strings: confirmed via `strings target/xtensa-esp32s3-espidf/release/zenclaw-agent | grep tool_call_started`.

### Build & test
- `cargo build --no-default-features --features desktop` ✓
- `cargo test --no-default-features --features desktop --lib` — 140 passed, 0 failed
- `just build devkitc` ✓
- `npm run build` (web) ✓

## Outstanding issue: ESP32 LLM call hangs

### Symptom
After the latest flash (commit `a4f489f`):
- `/ws/chat`: emits `thinking_started`, then nothing for >2min.
- `/api/chat`: same — POST hangs at 60s timeout.
- Device serial log shows the chat enters the runner: `LLM call: model=glm-5.1 body=11567B`. Then no further activity.
- `/api/status` and other non-chat endpoints respond instantly — the device isn't crashed; the agent_thread is just stuck inside the runner's HTTPS call.

### Was working
- **Before the second flash** (i.e. firmware with bug #1 + bug #2), REST `/api/chat` returned a real reply: `"Hey! 👋 I'm ZenClaw, your AI assistant running on an ESP32 embedded device..."` in ~5s.
- **Desktop binary on the same z.ai provider** returns `"Hey! 👋 I'm ZenClaw, running on your nucbox..."` in 5.8s.

So z.ai itself is fine. The hang is device-specific and started with the latest flash.

### Hypotheses (ranked)

1. **Telegram-unification commits broke ESP32 boot resources.** Two commits on this branch — `432516c` (refactor: hoist Channel trait + add EspHttpClient) and `2683b16` (feat: unify Telegram path) — were authored by you in parallel while I was working. They construct a permanent `Arc<dyn HttpClient>` at boot regardless of Telegram being enabled, and change `agent_thread`'s signature to take `http: Arc<dyn HttpClient>` and `tg: Option<...>`. They don't touch `esp32/runner.rs` but they could plausibly affect TLS resource initialization at boot. **Test:** rebase to drop those two commits, rebuild, reflash, retry chat.
2. **agent_thread routing breaks something subtle.** My commit `a4f489f` extracted `run_chat_request` and switched the call from `gateway.chat()` to `gateway.chat_with_events(..., events_tx.as_ref())`. Functionally equivalent (chat is now a wrapper). But the async state machine captures one more `Option<&Sender>` parameter; in theory the future's stack frame is slightly larger. **Test:** revert just `a4f489f` and reflash. If REST then works again, the routing change is implicated.
3. **Build cache mismatch.** `just clean devkitc && just build devkitc && just flash devkitc /dev/ttyACM1` — explicit clean-build to rule out a stale esp-idf-sys cache (called out in `CLAUDE.md` as a real failure mode).
4. **Power/USB instability.** DevKitC USB-C in some environments doesn't supply clean 5V; an HTTPS request that pulls more current than the supply can deliver could stall mbedtls. Worth checking the USB hub/power.

### Recommended next steps (in order of cost)

1. **Cheap test first** — `just clean devkitc && just build devkitc && just flash devkitc /dev/ttyACM1`. If REST `/api/chat` POST returns a reply in ~5s, the issue was a build cache; retry WS streaming.
2. If still hung, test hypothesis 2: revert `a4f489f`, rebuild, reflash. Drives a probe at the routing change without dropping the Telegram commits.
3. If still hung, test hypothesis 1: `git rebase --onto e62720e 2683b16 feat/streaming-chat-events` to drop the two Telegram commits, rebuild, reflash. (Be careful — those commits were yours; you may want to keep them on a separate branch first.)
4. If still hung, attach `espflash monitor --port /dev/ttyACM1` while a chat is running and watch for any mbedtls/heap warnings. The hang location is between `LLM call: model=glm-5.1 body=11567B` and any response — instrument inside `agent/src/esp32/runner.rs::esp_http_post` to find the exact blocking call.

## Branch state

```
* a4f489f fix(esp32): unblock /ws/chat — buffer trim + agent_thread routing
* c20a71e chore(desktop): support ZENCLAW_PORT + gitignore local config
* 9509b9f feat(web): render streamed tool calls as inline disclosures
* ad614a6 feat(desktop): stream ChatEvents over /ws/chat
* fbafd86 feat(esp32): stream ChatEvents over /ws/chat
* 81420db feat(core): emit typed ChatEvents through agent loop
* 2683b16 feat: unify Telegram path through Channel + HttpClient traits   ← parallel work
* 432516c refactor: hoist Channel trait + add EspHttpClient                ← parallel work
* 1182195 docs: design spec for streaming chat events
* e62720e (main, origin merge-base) docs: add implementation plan for Telegram path unification
```

`main` is at `e62720e`. Origin/main is at `ee67724` (main itself is 2 ahead of origin). Branch `feat/streaming-chat-events` has not been pushed.

## Key files

| Concern | File |
|---|---|
| Event types | `agent/src/core/chat_events.rs` |
| Gateway plumbing | `agent/src/core/gateway.rs` (`chat_with_events`) |
| Agent loop emission | `agent/src/core/agent_loop.rs` (`run_loop`, `execute_tool_calls`) |
| ESP32 transport | `agent/src/main.rs` (`/ws/chat` handler ~1240, `/api/chat/history` ~720, `agent_thread` ~1430, `run_chat_request` ~1525) |
| ESP32 runner (HTTPS) | `agent/src/esp32/runner.rs` (`esp_http_post`) |
| Desktop transport | `agent/src/desktop/server.rs` (`handle_chat_ws` ~492, `api_chat_history` ~176, `synthesize_history_events` ~210) |
| Desktop port env var | `agent/src/desktop/run.rs:84` (`ZENCLAW_PORT`) |
| Web types | `web/app/types/connection.ts` (`ChatEvent` union) |
| Web composable | `web/app/composables/useConnection.ts` (`openChatStream`, `getChatHistory`) |
| Web chat page | `web/app/pages/chat.vue` |
| Spec | `docs/superpowers/specs/2026-05-01-streaming-chat-events-design.md` |

## Live environment state

- **Web dev server:** Nuxt on `:3000` (the user's existing dev server).
- **Desktop binary:** `pid=2025284`, running my new code from `/home/ben/repos/zenclaw/agent/target/release/zenclaw-agent`, cwd `/home/ben/zenclaw-desktop`, port `8080`. Loaded `/home/ben/zenclaw-desktop/config.json` (z.ai provider configured). Logs at `/tmp/zenclaw-agent.log`.
- **Device:** `zenclaw-s3.local` (192.168.50.93), DevKitC, MAC `30:30:f9:16:8f:ec`. Serial at `/dev/ttyACM1`. Firmware = commit `a4f489f`. WS reaches `thinking_started`; LLM call hangs.
- **Local config symlink:** `agent/config.json` → `/home/ben/zenclaw-desktop/config.json`. Gitignored.

## Quick replay

To test the desktop path right now (no device involved):
```bash
# WS client check:
python3 -c "
import asyncio, json, websockets
async def main():
    async with websockets.connect('ws://127.0.0.1:8080/ws/chat') as ws:
        await ws.send(json.dumps({'type':'user_message','chat_id':'web','text':'hi'}))
        for _ in range(20):
            print(await asyncio.wait_for(ws.recv(), timeout=15))
asyncio.run(main())
"
```

Or open the web UI at `http://localhost:3000`, connect to `localhost` port `8080` (uncheck TLS), and chat — tool disclosures should render inline.

## Configuration so future sessions don't need keys

`agent/config.json` is a symlink → `/home/ben/zenclaw-desktop/config.json` (where the user's real keys live). Both are gitignored at repo root. Config has z-ai provider, Brave search, R2 storage all configured. Telegram is empty (would need a token if testing Telegram-channel paths).

`ZENCLAW_PORT` env var lets a test instance run on a different port (e.g. `ZENCLAW_PORT=8085 cargo run --features desktop`) without disturbing the primary on `:8080`.
