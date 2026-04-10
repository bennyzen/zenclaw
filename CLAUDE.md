# ZenClaw

AI agent framework for MicroPython. Targets ESP32 hardware and the MicroPython unix port for desktop development.

## Quick Start

```bash
# Desktop (MicroPython unix port)
cd firmware && micropython -X heapsize=4m run.py

# Programmatic test (LLM-to-LLM)
cd firmware && micropython -X heapsize=4m chat_test.py --reset "your message"
cd firmware && micropython -X heapsize=4m chat_test.py --session mytest --quiet "message"

# Tool smoke tests (no LLM, direct calls)
cd firmware && micropython -X heapsize=4m test_tools.py
```

### ESP32-S3 Deployment

**Preferred: Web UI provisioning (complete, handles everything)**

Open [bennyzen.github.io/zenclaw](https://bennyzen.github.io/zenclaw/) in Chrome or Edge (Web Serial required). The provisioning wizard flashes MicroPython + LittleFS filesystem + NVS (WiFi creds) and pushes the API key config — all in one shot. No CLI tools needed.

**Alternative: Build + Flash via CLI**

```bash
# 1. Build the LittleFS image (includes all firmware/ files + data/SOUL.md, data/AGENTS.md)
./scripts/build-firmware-image.sh
# Output: web/public/firmware/zenclaw.img (14MB)

# 2. Flash via CLI: enter bootloader mode first (hold BOOT + press RESET on the board)
#    PID changes from 303a:4001 (app) to 303a:0002 (bootloader)
esptool --port /dev/ttyACM0 --chip esp32s3 write-flash 0x200000 web/public/firmware/zenclaw.img
#    Press RESET after flashing to boot into application mode
```

Requires: `littlefs-python` (`pipx install littlefs-python`), `esptool` (`pipx install esptool`)

Note: CLI flashing only writes the filesystem image. You still need to flash MicroPython separately and provision WiFi credentials + config.json manually. The web UI handles all of this automatically.

**Alternative: mpremote cp (individual files, device must be at REPL)**

The device must be at the MicroPython REPL (not running main.py) for `mpremote cp` to work. If main.py is running, `mpremote` will try to interrupt it with Ctrl+C first — this works sometimes but can hang if the asyncio loop doesn't yield.

```bash
# Upload individual files (device must be idle/at REPL)
mpremote cp firmware/agent/runner.py :agent/runner.py

# Full deploy (from scratch or after filesystem format)
mpremote cp -r firmware/agent/ :agent/
mpremote cp -r firmware/lib/ :lib/
mpremote cp -r firmware/data/ :data/
mpremote cp -r firmware/stubs/ :stubs/
mpremote cp firmware/boot.py firmware/main.py firmware/config.json firmware/zenclaw_paths.py firmware/firmware-version.json :

# Reset to start
mpremote reset
```

**WiFi credentials** are stored in NVS (survives both methods). Provision once:
```bash
mpremote run firmware/provision_wifi.py
```

### Deploy, Test & Iterate on ESP32

The device is on `/dev/ttyACM0` (PID 0x4001 = application mode). After flashing, wait for boot (~12s for WiFi + API server), then smoke-test:

```bash
# Smoke-test API
curl -sf http://192.168.50.93/api/status | python3 -m json.tool

# Test chat (REST, non-streaming)
curl -sf --max-time 60 http://192.168.50.93/api/chat \
  -H 'Content-Type: application/json' -d '{"message":"ping"}'

# Run code directly on device (interrupts main.py — reset after)
mpremote exec "print('hello from ESP32')"
```

**Dev mode**: The ESP32 defaults to TLS on port 8443. After every flash/reset, if the web UI connects via HTTP (dev mode), toggle dev mode from the landing page or set `api.tls: false` in config.json. The web UI auto-detects HTTP vs HTTPS.

**Web UI testing with Playwright**: The Nuxt dev server runs on `http://localhost:3000`. Connect to the device by filling the hostname field on the Dashboard and clicking Connect. The Playwright MCP tools (`browser_navigate`, `browser_click`, `browser_type`, `browser_snapshot`, `browser_evaluate`) can drive the full UI. Use `browser_evaluate` to test WebSocket endpoints directly:

```js
// Example: test ws/chat from browser console
() => { return new Promise((resolve) => {
  const ws = new WebSocket('ws://192.168.50.93/ws/chat');
  ws.onopen = () => ws.send(JSON.stringify({message: 'ping', chat_id: 'test'}));
  ws.onmessage = (e) => { resolve(JSON.parse(e.data)); ws.close(); };
}); }
```

**Important**: `mpremote exec` interrupts `main.py` — always `mpremote reset` after. HMR reloads in Nuxt dev reset reactive state (connection drops) — reconnect after file saves.

WiFi credentials are stored in NVS (Non-Volatile Storage) and survive firmware reflash. Use `firmware/provision_wifi.py` to set/show/clear credentials:
```python
import provision_wifi
provision_wifi.setup()           # interactive SSID/password prompt
provision_wifi.show()            # show stored SSID
provision_wifi.clear()           # remove credentials
```

## Architecture

```
firmware/boot.py (ESP32 only)             WiFi from NVS -> connect
  -> firmware/lib/wifi.py                 NVS credential read, network.WLAN connect

firmware/main.py (ESP32) / firmware/run.py (desktop)
  -> firmware/agent/gateway.py            (ZenClawGateway singleton, config, lifecycle)
    -> firmware/agent/prompt.py           (system prompt from SOUL.md, tools, skills)
    -> firmware/agent/agent_loop.py       (LLM <-> tool execution loop)
      -> firmware/agent/runner.py         (provider dispatch, retry, streaming)
      -> firmware/agent/providers/        (Gemini/OpenAI/Anthropic API calls)
      -> firmware/agent/tools/            (consolidated action-param tools, lazy-loaded)
    -> firmware/agent/session_manager/    (JSONL conversation tree persistence)
    -> firmware/agent/heartbeat_runner.py (autonomous background loop)
```

### Key Modules

| Module | Purpose |
|--------|---------|
| `gateway.py` | Core orchestrator. Config loading, tool init, `chat()` entry point |
| `agent_loop.py` | `run_loop()` — LLM call -> tool execution -> repeat until text response |
| `runner.py` | Provider dispatch with retry. Selects model, handles streaming |
| `providers/__init__.py` | HTTP calls to Gemini/OpenAI. Parses tool calls from responses |
| `tools/__init__.py` | `ZenClawTools` class. Registers tools, dispatches `execute()` |
| `session_manager/manager.py` | JSONL-based branching conversation trees |
| `prompt.py` | Builds system prompt from SOUL.md, tools, skills, runtime info |
| `tool_loop.py` | Circuit breaker for stuck tool-use loops |
| `history.py` | Conversation history turn limiting |
| `workspace.py` | Loads bootstrap files (SOUL.md, AGENTS.md) from data/ |

### Channel System

Two delivery channels: `cli` (stdout via webrepl_binary) and `telegram`. The channel string flows through:
`gateway.chat(channel=)` -> `outbound.deliver(channel=)` -> `channels/outbound/{cli,telegram}.py`

### Telegram

The Telegram channel uses long-polling (`telegram/poller.py`) to receive messages and sends replies via the Bot API (`telegram/send.py`). On message receipt, a typing indicator is shown immediately. For DMs, replies are sent as final messages (no streaming/drafts). For group chats, the stream writer uses edit-based streaming. The poller is paused during chat processing to avoid duplicate handling.

Config requires `channels.telegram.enabled: true`, `bot_token` (from BotFather), and `default_chat_id`. Optional: `allowed_chat_ids` (list) to restrict access, `stream_debounce_ms` for group streaming.

### Session System

Each `chat_id` gets a JSONL file at `data/sessions/{chat_id}.jsonl`. The session manager supports branching conversation trees with compaction summaries. Session state (turn count, last channel, model override) lives in `agent/session.py`.

## Project Structure

```
zenclaw/
  firmware/                 ESP32 firmware (MicroPython agent)
    boot.py                   ESP32 boot (WiFi from NVS)
    main.py                   ESP32 entry point (starts agent)
    run.py                    Desktop entry point (interactive REPL)
    chat_test.py              Programmatic LLM test harness
    test_tools.py             Tool smoke tests (all should pass)
    provision_wifi.py         WiFi credential provisioning (NVS)
    config.example.json      Config template (copy to config.json with your keys)
    zenclaw_paths.py         Data directory paths (DATA_DIR, SESSIONS_DIR, etc.)
    firmware-version.json     Version metadata (platform: "0.1.0")

    agent/
      gateway.py              Core orchestrator
      agent_loop.py           LLM <-> tool loop
      runner.py               Provider dispatch + retry
      prompt.py               System prompt builder
      providers/__init__.py   Gemini/OpenAI API calls
      session.py              Per-chat session state
      history.py              Turn limiting
      workspace.py            Bootstrap file loading
      memory.py               Vector memory store
      outbound.py             Response delivery
      tool_loop.py            Circuit breaker
      commands.py             Slash command handling (/new, /reset, etc.)
      heartbeat_runner.py     Autonomous background loop
      ...                     (20+ more support modules)

      tools/                  Consolidated tools (action-param pattern, lazy-loaded)
        __init__.py           ZenClawTools registry + lazy loader (imports on first execute)
        file_tools.py         read, write, edit, list_dir
        exec_tool.py          exec (code execution with print capture)
        memory_tools.py       memory (action: save/search/get/reindex)
        cron_tools.py         cron (action: add/list/remove/run/update)
        web_tools.py          web_fetch, web_search, hub_search, hub_install
        session_tools.py      session (action: status/list/history)
        gateway_tool.py       gateway (action: status/reload)
        message_tool.py       message_send (cross-channel delivery)
        subagent_tools.py     subagents, sessions_spawn
        mcp_tools.py          mcp (action: connect/list_tools/call/disconnect/servers)
        gsheets_tools.py      Google Sheets (conditional: only if google.client_id configured)
        skill_tools.py        skill (action: run/stop/browse)
        sensor_tools.py       sense (hardware sensors, not registered for headless)
        storage_tools.py      storage (action: read/write/delete/list/info/grep/analyze)
        storage_heavy.py      Lazy-loaded heavy storage operations

      session_manager/        JSONL conversation persistence
      subagents/              Background agent spawning
      cron/                   Scheduled task execution
      telegram/               Telegram bot integration
      channels/               Channel abstraction (cli, telegram)

    lib/
      wifi.py                 WiFi connection manager (NVS credentials)
      httpclient.py           HTTP get/post/stream for MicroPython
      sys/log.py              log(level, msg, source=) logging
      sys/bg_tasks.py         Async background task management
      sys/board.py            Hardware detection (ESP32-S3 / desktop)
      sys/settings.py         Persistent settings (NVS on ESP32, memory on desktop)
      sys/storage.py          Storage detection (SD card / internal flash)

    stubs/                    MicroPython compatibility stubs
      webrepl_binary.py       CLI output capture
      network.py              Network module stub
      ujson.py                ujson -> json alias

    data/                     Runtime data (bootstrap files tracked, rest gitignored)
      SOUL.md                 Agent identity (tracked)
      AGENTS.md               Startup checklist (tracked)
      AGENTS.md               Startup checklist
      sessions/               Conversation JSONL files
      memory/                 Vector memory + index
      cron/jobs.json          Scheduled jobs
      skills/                 Installed skills
      state/                  Flush state, etc.

  web/                        Nuxt web UI (PWA dashboard, config editor, file manager, provisioning)
```

## Coding Conventions

### First Principle: Keep It Slim

This code runs on a microcontroller with limited flash, RAM, and CPU. Do not bloat the codebase with workarounds, compatibility shims, converters, or defensive abstractions. Think deeply about the simplest solution. If a feature doesn't need processing, don't process it. If a library handles something natively, don't wrap it. Every byte counts.

### Imports

- **Within `firmware/agent/`**: Use relative imports (`from .session import get_session`, `from ..tools import ZenClawTools`) except for `zenclaw_paths` which must be absolute (`import zenclaw_paths`) — no symlinks on ESP32
- **To `firmware/lib/`**: Use absolute (`from lib.sys.log import log`, `from lib.httpclient import post`)
- **To root modules**: Use absolute (`import zenclaw_paths`, `from zenclaw_paths import SESSIONS_DIR`)
- **MicroPython compat**: `try: import asyncio` / `except: import uasyncio as asyncio`

Note: Internal imports within the firmware use paths relative to `firmware/` (e.g., `from lib.sys.log import log`, not `from firmware.lib.sys.log import log`) because MicroPython runs from inside the `firmware/` directory.

### Async Pattern

All coroutines use `async def` + `await`. Do not use `yield from` for async calls.

### Tool Registration

Tools use the **action-param pattern** — each module registers one tool with an `action` parameter that selects the operation. This reduces tool count for the LLM and saves RAM through fewer schema entries.

```python
def create_my_tools(config):
    async def _do_read(args):
        return 'read result'

    async def _do_write(args):
        content = args.get('content', '')
        return 'wrote {} bytes'.format(len(content))

    async def _my_tool(args):
        action = args.get('action', 'read')
        if action == 'read':
            return await _do_read(args)
        if action == 'write':
            return await _do_write(args)
        return "Unknown action '{}'".format(action)

    return {
        'my_tool': {
            'description': 'My tool. Actions: read, write.',
            'parameters': {
                'type': 'object',
                'properties': {
                    'action': {'type': 'string', 'enum': ['read', 'write']},
                    'content': {'type': 'string', 'description': 'Content (write)'},
                },
                'required': ['action'],
            },
            'execute': _my_tool,
        },
    }
```

Register in `firmware/agent/tools/__init__.py` by adding the module to `_TOOL_MODULES`.

**Lazy loading**: `__init__.py` imports each module at boot only to extract schemas, then releases the module from `sys.modules`. The actual bytecode is garbage-collected and only re-imported on first `execute()`. This saves ~130KB of RAM on ESP32 boards without PSRAM.

### Logging

```python
from lib.sys.log import log
log('info', 'MESSAGE', source='zenclaw')
log('error', 'FAILED: {}'.format(e), source='zenclaw')
```

Always use positional args (`log('info', msg)`) not keyword (`log(info=msg)`).

### Paths

All data paths go through `zenclaw_paths`:
```python
import zenclaw_paths
path = '{}/myfile.json'.format(zenclaw_paths.DATA_DIR)
```

Never hardcode `data/` or `/zenclaw/` paths.

### MicroPython Compatibility

- No `os.environ` — hardcode or use config.json
- `time.ticks_ms()` / `time.ticks_diff()` need try/except fallback to `int(time.time() * 1000)`
- `asyncio.sleep_ms(N)` -> use `asyncio.sleep(N/1000)` instead
- `sys.print_exception(e)` needs fallback to `traceback.print_exc()`
- No f-strings — use `'{}'.format(x)`
- `gc.mem_free()` / `gc.mem_alloc()` may not exist on all platforms

## Config (firmware/config.json)

Copy from `firmware/config.example.json` and fill in your API keys. Config.json is gitignored to prevent secret leaks.

```json
{
  "providers": {
    "default": "google",
    "google": {
      "api_key": "...",
      "model": "gemini-2.5-flash",
      "base_url": "https://generativelanguage.googleapis.com/v1beta"
    }
  },
  "agent_name": "ZenClaw",
  "heartbeat": { "enabled": false },
  "channels": {
    "telegram": {
      "enabled": true,
      "bot_token": "...",
      "default_chat_id": "..."
    }
  }
}
```

Provider `base_url` determines the API format: Gemini URLs use Gemini wire format, everything else uses OpenAI-compatible format. Gemini auth uses `?key=` in URL (no Bearer header).

## ESP32-S3 Hardware

### Boot Sequence

1. `boot.py` — reads WiFi SSID/password from NVS, calls `network.WLAN().connect()`
2. `main.py` — initializes paths, loads gateway, runs headless (asyncio event loop for Telegram + heartbeat)

### NVS Storage

WiFi credentials use the `wifi` NVS namespace with keys `ssid` and `password`. Settings use the `settings` namespace. NVS data survives firmware reflash and filesystem format.

### Platform Detection

`firmware/lib/sys/board.py` auto-detects the platform on import:
- **ESP32**: reads `machine.freq()`, `esp32.mcu_temperature()`, PSRAM, capabilities
- **Desktop**: returns stub data (`x86_64`, no capabilities)

Use `board.is_esp32()` to check platform at runtime.

### File Layout on ESP32

Files from `firmware/` are uploaded to the root of the ESP32 filesystem (`/`). MicroPython's default `sys.path` includes `/` and `/lib`, so imports work without the stubs path that `run.py` uses on desktop. The `firmware/` prefix is only for repo organization — it does not exist on the device.

### Memory Considerations

ESP32-S3 typically has 512 KB SRAM + optional 2-8 MB PSRAM. The agent must be memory-conscious:
- Session JSONL files can grow large — compaction keeps them bounded
- `gc.collect()` before heavy operations
- The circuit breaker in `tool_loop.py` prevents runaway tool loops

## Common Pitfalls

- **Tool execute signature**: Tools receive a single `args` dict. The executor injects `_chat_id` and `_prompt_source` (underscore-prefixed). Don't use `**kwargs` or keyword-only params.
- **Gemini auth**: The Gemini provider appends `?key=API_KEY` to the URL. Do NOT also send a `Bearer` authorization header — they're mutually exclusive and Gemini returns 401 if both are present.
- **Session history poisoning**: If the LLM encounters repeated tool failures in its conversation history, it may hallucinate the same error without retrying. Clear the session file to reset.
- **Telegram DM delivery**: Do not use the stream writer (TelegramStreamWriter) for DMs — Telegram has no draft API for bots. DMs use direct `send_message()` delivery. The stream writer is only for group chats (edit-based streaming).
- **ESP32 event loop**: `main.py` must never call `input()` or other blocking calls. The asyncio event loop must stay free for background tasks (Telegram poller, heartbeat). Use `await asyncio.sleep()` to yield.
