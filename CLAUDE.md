# ZenClaw

AI agent framework. Two implementations: a **Rust agent** (`agent-esp32/`) targeting ESP32-S3 hardware (active development), and a **MicroPython agent** (`firmware/`) for MicroPython on ESP32 + desktop.

## Rust Agent (`agent-esp32/`)

The Rust agent is the primary ESP32-S3 target. Built with `esp-idf-svc` (ESP-IDF v5.4), no tokio — uses `block_on` for async and `std::thread` for concurrency.

### Quick Start

```bash
cd agent-esp32

# Build for default board (DevKitC with PSRAM)
cargo build --release

# Build for a specific board profile (IMPORTANT: must match hardware!)
ESP_IDF_SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.board.sdcard" cargo build --release

# Flash (ALWAYS use --bootloader flag — bundled bootloader causes boot loops)
espflash flash --port /dev/ttyACM0 --partition-table partitions.csv --bootloader bootloader.bin target/xtensa-esp32s3-espidf/release/zenclaw-agent

# Monitor serial output
espflash monitor --port /dev/ttyACM0 --non-interactive
```

### Board Profiles

The board profile MUST match the hardware. Flashing a PSRAM-enabled build onto a board without PSRAM will crash at boot (`Failed to init external RAM!`).

Board profiles are layered via `ESP_IDF_SDKCONFIG_DEFAULTS` (semicolon-separated):

| Profile | File | Hardware | Key Config |
|---------|------|----------|------------|
| **DevKitC** | `sdkconfig.board.devkitc` | ESP32-S3-DevKitC (2x USB, 8MB PSRAM) | `CONFIG_SPIRAM=y`, UART console, USB Host enabled |
| **SD Card** | `sdkconfig.board.sdcard` | LILYGO T-Dongle-S3 (1x USB, no PSRAM, SD slot) | USB Serial/JTAG console, no SPIRAM |

Default board is set in `.cargo/config.toml` (`ESP_IDF_SDKCONFIG_DEFAULTS`). Override with env var.

**CRITICAL**: When switching board profiles, the esp-idf-sys build cache may retain the old sdkconfig. If the board doesn't boot, clean and rebuild:
```bash
rm -rf target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*
ESP_IDF_SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.board.sdcard" cargo build --release
```

### WiFi & Config Provisioning (NVS)

WiFi credentials and config are stored in NVS (survives reflash). Provision via `espflash`:

```bash
# WiFi credentials (namespace: "wifi", keys: "ssid" and "password")
espflash write-nvs --port /dev/ttyACM0 wifi.csv
# wifi.csv format:
# key,type,encoding,value
# wifi,namespace,,
# ssid,data,string,YOUR_SSID
# password,data,string,YOUR_PASSWORD

# Config is provisioned via /api/config POST after WiFi connects (triggers reboot)
curl -X POST http://zenclaw.local/api/config \
  -H 'Content-Type: application/json' \
  -d '{"providers":{"default":"google","google":{"api_key":"...","model":"gemini-2.5-flash"}}}'
```

### Deploy, Test & Iterate

After flashing, wait ~12s for WiFi + HTTP server, then:

```bash
# Smoke-test
curl -sf http://zenclaw.local/api/status | python3 -m json.tool

# Chat
curl -sf --max-time 60 http://zenclaw.local/api/chat \
  -H 'Content-Type: application/json' -d '{"message":"ping"}'

# Chat history
curl -sf "http://zenclaw.local/api/chat/history?chat_id=web" | python3 -m json.tool
```

**Web UI**: Nuxt dev server at `http://localhost:3000`. Connect to device by hostname on the Dashboard. Playwright MCP tools can drive the full UI.

### Architecture

```
agent-esp32/src/
  main.rs                     ESP32 entry: WiFi, mDNS, SPIFFS, HTTP server, Telegram poller
  lib.rs                      Feature-gated module exports
  config.rs                   Config structs (serde, mirrors firmware/config.json shape)
  usb_storage.rs              USB Host MSC FFI wrapper (feature: usb_storage)

  core/                       Shared agent logic
    gateway.rs                Core orchestrator, chat() entry point
    agent_loop.rs             LLM <-> tool execution loop
    runner.rs                 Provider dispatch trait
    prompt.rs                 System prompt builder
    types.rs                  Shared types (Message, ToolCall, etc.)
    tool_loop.rs              Circuit breaker
    workspace.rs              Bootstrap file loading
    telegram.rs               Telegram bot (long-poll + send)
    subagents.rs              Background agent spawning
    cron.rs                   Scheduled tasks
    tools/                    Tool implementations
    sessions/                 JSONL conversation persistence
    channels/                 Channel abstraction
    background/               Background task management
    memory/                   Vector memory store

  esp32/                      ESP32-specific implementations
    mod.rs                    Module exports
    runner.rs                 EspRunner — HTTP calls via esp-idf-svc

  platform/                   Platform abstraction (HTTP client/server, runtime)
  desktop/                    Desktop-only (axum server, reqwest client)
```

### HTTP API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Landing page (agent name, IP, heap, version) |
| GET | `/api/status` | System status (memory, WiFi, storage, temp, channels, provider, USB) |
| POST | `/api/chat` | Send message `{"message":"...", "chat_id":"web"}` |
| GET | `/api/chat/history?chat_id=` | Conversation history for chat_id |
| GET/POST | `/api/config` | Read/write config (POST triggers reboot) |
| GET | `/api/files?path=` | List directory |
| GET | `/api/files/read?path=` | Read file content |
| POST | `/api/files/write` | Write file `{"path":"...", "content":"..."}` |
| POST | `/api/files/mkdir` | Create directory `{"path":"..."}` |
| POST | `/api/files/upload` | Upload file (multipart) |
| POST | `/api/restart` | Reboot device |
| GET | `/api/wifi` | WiFi info (SSID, RSSI) |
| WS | `/ws/chat` | WebSocket chat streaming |
| WS | `/ws/logs` | WebSocket log streaming |

### Features & Cargo Features

| Feature | Description |
|---------|-------------|
| `esp32` (default) | ESP32 target — esp-idf-svc, embedded-svc |
| `desktop` | Desktop target — tokio, axum, reqwest |
| `usb_storage` | USB Host MSC support (requires `esp32`, DevKitC board + powered USB hub) |
| `hnsw` | HNSW vector index via usearch |

### Partition Table

```
nvs       0x9000   24KB   — WiFi creds, config JSON, settings
phy_init  0xf000   4KB    — RF calibration
factory   0x10000  4MB    — Application binary
storage   0x410000 8MB    — SPIFFS (sessions, memory, data files)
```

### Common Pitfalls (Rust)

- **Board profile mismatch**: Flashing a PSRAM-enabled build onto no-PSRAM hardware crashes at boot before any Rust code runs. Always verify `CONFIG_SPIRAM` matches hardware.
- **Bootloader flag**: Always pass `--bootloader bootloader.bin` to `espflash flash`. The bundled bootloader causes boot loops on ESP32-S3.
- **Build cache**: `esp-idf-sys` caches the sdkconfig. Changing board profile without cleaning `target/.../build/esp-idf-sys-*` has no effect.
- **No tokio on ESP32**: The ESP32 feature uses `esp_idf_svc::hal::task::block_on` for async and `std::thread` for concurrency. Do not add tokio.
- **NVS erase**: Never use `espflash erase-flash` — it wipes NVS (WiFi creds + config). Flash specific partition offsets instead.
- **USB PHY sharing**: ESP32-S3 has one USB PHY shared between Serial/JTAG and OTG. `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` claims it, blocking USB Host. DevKitC uses UART console to free the PHY.
- **USB Host VBUS**: DevKitC USB-C port doesn't supply 5V in host mode. USB devices need a powered hub.
- **Main thread must not block**: The main thread parks in `loop { sleep(60s) }` after spawning HTTP server and Telegram poller threads. HTTP server runs in esp-idf's httpd thread pool.

## MicroPython Agent (`firmware/`)

The original MicroPython implementation. Still functional for ESP32 + desktop development.

### Quick Start

```bash
# Desktop (MicroPython unix port)
cd firmware && micropython -X heapsize=4m run.py

# Programmatic test (LLM-to-LLM)
cd firmware && micropython -X heapsize=4m chat_test.py --reset "your message"

# Tool smoke tests (no LLM, direct calls)
cd firmware && micropython -X heapsize=4m test_tools.py
```

### ESP32-S3 Deployment (MicroPython)

**Preferred: Web UI provisioning** — Open [bennyzen.github.io/zenclaw](https://bennyzen.github.io/zenclaw/) in Chrome/Edge. Flashes MicroPython + LittleFS + NVS in one shot.

**Alternative: Build + Flash via CLI**

```bash
./scripts/build-firmware-image.sh
esptool --port /dev/ttyACM0 --chip esp32s3 write_flash 0x200000 web/public/firmware/zenclaw.img
```

**Alternative: mpremote cp** (device must be at REPL, not running main.py)

### Architecture (MicroPython)

```
firmware/boot.py (ESP32 only)             WiFi from NVS -> connect
firmware/main.py (ESP32) / firmware/run.py (desktop)
  -> agent/gateway.py                     (config, lifecycle, chat() entry point)
    -> agent/agent_loop.py                (LLM <-> tool execution loop)
      -> agent/runner.py                  (provider dispatch, retry, streaming)
      -> agent/providers/                 (Gemini/OpenAI/Anthropic API calls)
      -> agent/tools/                     (action-param tools, lazy-loaded)
    -> agent/session_manager/             (JSONL conversation tree persistence)
    -> agent/heartbeat_runner.py          (autonomous background loop)
```

### MicroPython Coding Conventions

- **Keep it slim**: Every byte counts on a microcontroller
- **Imports**: Relative within `agent/`, absolute to `lib/`. MicroPython compat: `try: import asyncio` / `except: import uasyncio as asyncio`
- **No f-strings**: Use `'{}'.format(x)`
- **Tool pattern**: Action-param pattern with lazy loading (see `firmware/agent/tools/__init__.py`)
- **Logging**: `from lib.sys.log import log; log('info', 'MESSAGE', source='zenclaw')`
- **Paths**: All through `zenclaw_paths` — never hardcode `data/` or `/zenclaw/`

## Shared Concepts

### Config Format

Same JSON format for both Rust and MicroPython agents:

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

Provider `base_url` determines API format: Gemini URLs use Gemini wire format, everything else uses OpenAI-compatible format. Gemini auth uses `?key=` in URL (no Bearer header).

### NVS Storage

WiFi credentials use the `wifi` NVS namespace with keys `ssid` and `password`. Config uses the `config` namespace with key `json`. NVS data survives firmware reflash and filesystem format.

### Telegram

Long-polling for inbound messages, Bot API for sends. Config requires `channels.telegram.enabled: true`, `bot_token`, and `default_chat_id`. Optional: `allowed_chat_ids` list, `stream_debounce_ms`.

### Session System

Each `chat_id` gets a JSONL file at `data/sessions/{chat_id}.jsonl` (or `/data/sessions/` on ESP32). Branching conversation trees with compaction summaries.

### Memory Considerations

ESP32-S3: 512 KB SRAM + optional 2-8 MB PSRAM. Boards without PSRAM (T-Dongle-S3) have ~175KB free heap after WiFi+TLS. Session compaction keeps JSONL files bounded. TLS alone needs ~40-50KB.

## Project Structure

```
zenclaw/
  agent-esp32/              Rust agent (ESP32-S3 + desktop targets)
    .cargo/config.toml        Build target + default board profile
    Cargo.toml                Dependencies, features, ESP-IDF components
    partitions.csv            Flash partition layout (NVS + 4MB app + 8MB SPIFFS)
    bootloader.bin            Pre-built bootloader (always use with espflash)
    sdkconfig.defaults        Shared ESP-IDF config (flash size, TLS, HTTP server)
    sdkconfig.board.devkitc   DevKitC profile (PSRAM, UART console, USB Host)
    sdkconfig.board.sdcard    T-Dongle-S3 profile (no PSRAM, USB Serial/JTAG)
    bindings_usb_msc.h        Bindgen header for USB Host MSC component
    src/                      Rust source (see Architecture above)

  firmware/                 MicroPython agent (ESP32 + desktop)
    boot.py                   ESP32 boot (WiFi from NVS)
    main.py                   ESP32 entry point
    run.py                    Desktop entry point (interactive REPL)
    agent/                    Agent core (gateway, tools, sessions, telegram)
    lib/                      Platform libraries (WiFi, HTTP, logging)
    data/                     Runtime data (SOUL.md, sessions, memory)

  web/                      Nuxt web UI (PWA dashboard, config editor, file manager, provisioning)
```

## Common Pitfalls (General)

- **Gemini auth**: Gemini uses `?key=API_KEY` in URL. Do NOT also send `Bearer` header — returns 401 if both present.
- **Session history poisoning**: Repeated tool failures in history can cause the LLM to hallucinate the same error. Clear the session file to reset.
- **ESP32 NVS**: Never `espflash erase-flash` — wipes WiFi creds and config. Flash specific offsets only.
