# ZenClaw

AI agent framework. Single Rust implementation (`agent/`) targeting ESP32-S3 and ESP32-P4 hardware, plus a desktop build for development. Multiple devices coexist on a single network — each gets a unique mDNS hostname (web UI provisioning sets it; CLI-flashed devices fall back to `zenclaw-XXYYZZ` derived from the lower 3 bytes of the MAC). The original MicroPython implementation has been retired and removed.

## Rust Agent (`agent/`)

The Rust agent targets ESP32-S3 and ESP32-P4 boards. Built with `esp-idf-svc` (ESP-IDF v5.4), no tokio — uses `block_on` for async and `std::thread` for concurrency.

### Quick Start

```bash
cd agent

# Build for DevKitC (ESP32-S3)
just build devkitc

# Build for Guition P4
just build guition-p4

# Flash (board manifest supplies the correct --bootloader automatically)
just flash devkitc /dev/ttyACM0
just flash guition-p4 /dev/ttyACM0

# List all available boards
just list

# Monitor serial output
espflash monitor --port /dev/ttyACM0 --non-interactive
```

### Board Profiles

The board profile MUST match the hardware. Flashing a PSRAM-enabled build onto a board without PSRAM will crash at boot (`Failed to init external RAM!`).

`just build <board>` is the canonical build path. It reads `boards/<name>.toml` to set the correct cargo target, sdkconfig stack, features, and bootloader automatically. The legacy `ESP_IDF_SDKCONFIG_DEFAULTS` env-var override still works for one-off builds but is no longer the default workflow.

| Board | Manifest | Hardware | Key Config |
|-------|----------|----------|------------|
| **devkitc** | `boards/devkitc.toml` | ESP32-S3-DevKitC (2x USB, 8MB PSRAM) | `CONFIG_SPIRAM=y`, UART console, USB Host enabled |
| **guition-p4** | `boards/guition-p4.toml` | Guition JC-ESP32P4-M3-DEV (Ethernet, 32MB PSRAM, microSD slot) | RISC-V target, IP101 PHY, FATFS + SDMMC via on-chip LDO_VO4, no WiFi provisioning needed |

**CRITICAL**: When switching board profiles, the esp-idf-sys build cache may retain the old sdkconfig. If the board doesn't boot, clean and rebuild:
```bash
just clean devkitc
just build devkitc
```

### Multi-board build system

Each board is described by a TOML manifest in `boards/<name>.toml`:

```toml
chip        = "esp32s3"                                  # chip family (used for bootloader lookup)
target      = "xtensa-esp32s3-espidf"                    # cargo build target
sdkconfig   = ["sdkconfig.defaults", "sdkconfig.board.devkitc"]  # ordered sdkconfig layers
bootloader  = "bootloaders/esp32s3.bin"                  # vendored bootloader
features    = ["esp32", "nic-wifi-internal"]             # cargo features (no-default-features implied)
default_baud = 921600                                    # espflash baud rate
description = "ESP32-S3-DevKitC (PSRAM, USB Host capable)"
```

- `just list` — prints all boards with descriptions
- `just build <board>` — builds with the correct target + sdkconfig + features
- `just flash <board> [port]` — flashes with the correct bootloader
- `just clean <board>` — wipes the esp-idf-sys cache for that target
- `bootloaders/<chip>.bin` are vendored from clean `esp-idf-sys` builds for each chip
- `agent-smoke/` is the minimal reference template for porting to new chips

### ESP32-P4 (Guition JC-ESP32P4-M3-DEV)

- **Target**: `riscv32imafc-esp-espidf` (RISC-V; no Xtensa toolchain needed)
- **Network**: Ethernet via IP101 PHY — plug in an Ethernet cable; no WiFi provisioning needed
- **Key pin map** (RMII bus):
  | Signal | GPIO |
  |--------|------|
  | TX_EN  | 49   |
  | TXD0   | 34   |
  | TXD1   | 35   |
  | CRS_DV | 28   |
  | RXD0   | 29   |
  | RXD1   | 30   |
  | MDC    | 31   |
  | MDIO   | 52   |
  | REF_CLK| 50   | (50 MHz input from PHY oscillator) |
  | PHY_PWR| 51   | (hw-reset GPIO) |
  | PHY_ADDR | 1  | |
- **Workflow**: `just build guition-p4 && just flash guition-p4 /dev/ttyACM0`
- **Discovery**: mDNS `zenclaw.local` works identically to S3 builds; boot to agent-ready in ~5s
- **Config**: provision via `/api/config` POST after Ethernet comes up (same as S3)
- **microSD slot** (`sdcard` feature, default-on for this board):
  - **Driver**: SDMMC peripheral, slot 0 IO_MUX path, 4-bit @ 20 MHz default-speed (1-bit fallback if 4-bit fails)
  - **Pinmap** (P4 IO_MUX defaults; the Guition wires the slot to these):
    | Signal | GPIO |
    |--------|------|
    | CLK    | 43 |
    | CMD    | 44 |
    | D0     | 39 |
    | D1     | 40 |
    | D2     | 41 |
    | D3     | 42 |
  - **Bus power**: on-chip LDO_VO4 (channel 4) — without `sd_pwr_ctrl_new_on_chip_ldo()` every CMD times out (`ESP_ERR_TIMEOUT 0x107`). See Common Pitfalls.
  - **Mount**: `/sdcard` (LittleFS still owns `/data`). `format_if_mount_failed: false` — never auto-format user data.
  - **Implementation**: `agent/components/zenclaw_sd/` (local IDF C component wrapping `SDMMC_HOST_DEFAULT()` + `esp_vfs_fat_sdmmc_mount()`) + `agent/src/sdcard.rs` (Rust wrapper). The C wrapper avoids hand-mirroring the 17 function pointers + anonymous union of `sdmmc_host_t` in Rust.
  - **API surface**: `/api/status.sdcard` reports `{mounted, path, total_kb, free_kb, type, bus_width}`; `/api/files*` accepts `/sdcard/...` paths (jail enforced via `jail_filesystem_path`); web file UI shows an SD tab when `mounted: true`.
- **C6 WiFi co-processor**: the onboard ESP32-C6 is held in reset; WiFi deferred to v2

### Provisioning

**Preferred: web UI wizard** — `web/app/pages/provision.vue` flashes the Rust agent over Web Serial (Chrome/Edge). Pick the board (DevKitC or Guition P4), enter a device name (dice button rolls a fresh `zenclaw-{adj}-{noun}`), WiFi creds (optional for Ethernet boards), provider/API key, click Flash. The wizard:

- Validates that the chip detected by esptool-js matches the chosen board (chip-mismatch is caught before any erase).
- Flashes the merged image at `0x0` and a NVS partition at `0x9000` containing `device/hostname` plus optional `wifi/ssid` + `wifi/password`.
- POSTs `/api/config` once the device announces on mDNS at `<hostname>.local`.

**Build the wizard's firmware artifacts:**
```bash
./scripts/build-rust-firmware.sh           # builds all 3 boards + firmware.json
./scripts/build-rust-firmware.sh devkitc   # rebuild a single board (manifest preserved)
```

**Multiple devices coexist** because each device's mDNS hostname is unique:
- Web-UI provisioned: hostname = whatever the user typed (or the dice-rolled name).
- CLI-flashed: hostname = `zenclaw-XXYYZZ` from the lower 3 bytes of the WiFi-STA MAC. Stable across reboots.

**Manual NVS provisioning** (for headless/CLI flows; equivalent to what the wizard writes):

```bash
# WiFi credentials (namespace: "wifi", keys: "ssid" and "password")
espflash write-nvs --port /dev/ttyACM0 wifi.csv
# wifi.csv format:
# key,type,encoding,value
# wifi,namespace,,
# ssid,data,string,YOUR_SSID
# password,data,string,YOUR_PASSWORD

# Optional: set a custom hostname (otherwise falls back to MAC-derived)
# Add to a separate device.csv with namespace "device" and key "hostname"

# Push provider config after the device is online:
curl -X POST http://<hostname>.local/api/config \
  -H 'Content-Type: application/json' \
  -d '{"providers":{"default":"google","google":{"api_key":"...","model":"gemini-2.5-flash"}}}'
```

### Deploy, Test & Iterate

After flashing, wait ~12s for network + HTTP server (S3: WiFi connect; P4: Ethernet DHCP ~5s), then (substitute your device's hostname):

```bash
HOST=zenclaw-swift-fox.local

# Smoke-test
curl -sf http://$HOST/api/status | python3 -m json.tool

# Chat
curl -sf --max-time 60 http://$HOST/api/chat \
  -H 'Content-Type: application/json' -d '{"message":"ping"}'

# Chat history
curl -sf "http://$HOST/api/chat/history?chat_id=web" | python3 -m json.tool
```

**Web UI**: Nuxt dev server at `http://localhost:3000`. Connect to a device by hostname on the Dashboard — multiple devices each have a unique `<name>.local` address. Playwright MCP tools can drive the full UI.

### Architecture

```
agent/src/
  main.rs                     ESP32 entry: WiFi, mDNS, LittleFS, SD card (P4), HTTP server, Telegram poller
  lib.rs                      Feature-gated module exports
  config.rs                   Config structs (serde, mirrors `/api/config` JSON shape)
  usb_storage.rs              USB Host MSC FFI wrapper (feature: usb_storage)
  sdcard.rs                   FATFS-on-SDMMC driver wrapper (feature: sdcard)

  core/                       Shared agent logic
    gateway.rs                Core orchestrator, chat() entry point
    agent_loop.rs             LLM <-> tool execution loop
    runner.rs                 Provider dispatch trait
    prompt.rs                 System prompt builder
    types.rs                  Shared types (Message, ToolCall, etc.)
    tool_loop.rs              Circuit breaker
    compaction.rs             Session compaction
    workspace.rs              Bootstrap file loading
    cron.rs                   Scheduled tasks
    tools/                    Tool implementations
    sessions/                 JSONL conversation persistence
    channels/                 Channel abstraction
      telegram.rs             Telegram bot (long-poll + send)
    cloud/                    S3-compatible client + SigV4 signer

  net/                        NIC abstraction (trait + per-driver modules)
    mod.rs                    Nic trait, IpInfo, bring_up_primary dispatch
    wifi.rs                   EspWifi driver (feature: nic-wifi-internal)
    wifi_ui.rs                NVS credential read/write + /api/wifi handlers
    eth.rs                    IP101 EMAC driver via raw FFI (feature: nic-eth)

  esp32/                      ESP32-specific implementations
    mod.rs                    Module exports
    runner.rs                 EspRunner — HTTP calls via esp-idf-svc

  platform/                   Platform abstraction (HTTP client/server, runtime)
  desktop/                    Desktop-only target (axum server, reqwest client)
    server.rs                 axum HTTP server
    run.rs                    Desktop entry point
    subagents.rs              Background agent spawning (desktop-only)
    background/               Background task management (cron/heartbeat, desktop-only)
```

### HTTP API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Landing page (agent name, IP, heap, version) |
| GET | `/api/status` | System status (memory, WiFi, storage, temp, channels, provider, USB, sdcard) |
| POST | `/api/chat` | Send message `{"message":"...", "chat_id":"web"}` |
| GET | `/api/chat/history?chat_id=` | Conversation history for chat_id |
| GET/POST | `/api/config` | Read/write config (POST triggers reboot) |
| GET | `/api/files?path=` | List directory (path under `/data` or, when mounted, `/sdcard`) |
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
| `nic-wifi-internal` | Native WiFi via EspWifi (S3/S2); enabled by devkitc board manifest |
| `nic-wifi-hosted` | WiFi via esp_hosted (C6/C5 SDIO co-proc) — v2, not yet implemented |
| `nic-eth` | Internal EMAC + external PHY (P4); enabled by guition-p4 board manifest |
| `usb_storage` | USB Host MSC support (requires `esp32`, DevKitC board + powered USB hub) |
| `sdcard` | microSD via FATFS+SDMMC; enabled by guition-p4 board manifest. P4 boards need on-chip LDO_VO4 (handled in `components/zenclaw_sd/`). |
| `hnsw` | HNSW vector index via usearch |

### Partition Table

```
nvs       0x9000   24KB   — WiFi creds, config JSON, settings
phy_init  0xf000   4KB    — RF calibration
factory   0x10000  4MB    — Application binary
storage   0x410000 8MB    — LittleFS (sessions, memory, data files)
```

### Common Pitfalls (Rust)

- **Board profile mismatch**: Flashing a PSRAM-enabled build onto no-PSRAM hardware crashes at boot before any Rust code runs. Always verify `CONFIG_SPIRAM` matches hardware.
- **Bootloader flag**: `just flash` always supplies the correct `--bootloader` from the board manifest. If calling `espflash` directly, always pass `--bootloader bootloaders/<chip>.bin`. The bundled bootloader causes boot loops.
- **Build cache**: `esp-idf-sys` caches the sdkconfig. Changing board profile without cleaning `target/.../build/esp-idf-sys-*` has no effect. Use `just clean <board>` to wipe it.
- **No tokio on ESP32**: The ESP32 feature uses `esp_idf_svc::hal::task::block_on` for async and `std::thread` for concurrency. Do not add tokio.
- **NVS erase**: Never use `espflash erase-flash` — it wipes NVS (WiFi creds + config). Flash specific partition offsets instead.
- **USB PHY sharing**: ESP32-S3 has one USB PHY shared between Serial/JTAG and OTG. `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` claims it, blocking USB Host. DevKitC uses UART console to free the PHY.
- **USB Host VBUS**: DevKitC USB-C port doesn't supply 5V in host mode. USB devices need a powered hub.
- **Main thread must not block**: The main thread parks in `loop { sleep(60s) }` after spawning HTTP server and Telegram poller threads. HTTP server runs in esp-idf's httpd thread pool.
- **S3 Xtensa LLVM bug (1.94.0.0–1.95.0.0)**: `xtensa-esp32s3-espidf` builds fail with `XtensaISD::PCREL_WRAPPER` LLVM ICE in `serde_json::Vec` deserialization on every `esp-rs/rust` release from 1.94.0.0 through 1.95.0.0 (tracked at [esp-rs/rust#277](https://github.com/esp-rs/rust/issues/277), regression introduced by the LLVM 20→21 bump). Workaround — pin the toolchain to 1.93.0.0: `espup install --toolchain-version 1.93.0.0`. P4 (RISC-V) is unaffected and uses the standard toolchain.
- **ESP32-P4 SDMMC needs the on-chip LDO**: P4's SDMMC peripheral does not internally power its IO bus — the chip exposes `VDDPST_5` and expects external power. Every P4 board (Espressif EV-Board, Guition, etc.) wires this to on-chip `LDO_VO4` and depends on firmware to enable it. Without `sd_pwr_ctrl_new_on_chip_ldo({.ldo_chan_id = 4})` + `host.pwr_ctrl_handle = ...`, every CMD times out: `sdmmc_init_ocr: send_op_cond (1) returned 0x107`. LDO channels 1 (flash) and 2 (PSRAM) are reserved; channel 4 is the SDMMC default. Wrap in `#if SOC_SDMMC_IO_POWER_EXTERNAL` for chip portability — S3 doesn't need it. Confirmed against upstream micropython issue [#18984](https://github.com/micropython/micropython/issues/18984).
- **C-component edits don't always trigger ESP-IDF rebuild**: editing a C file inside `agent/components/<name>/` may leave the .obj timestamp stale despite a successful `cargo build` — the binary then lacks the change. Symptom: `just build` finishes in ~20s, but `strings target/.../zenclaw-agent | grep <new-log-message>` returns nothing. Fix: `just clean <board>` between iterations on C-component code. Same rule that applies to sdkconfig/`extra_components` additions, just stated more generally.

### Memory subsystem

Memory is a single text file at `data/MEMORY.md` with `## [id] timestamp (tags: ...)` blocks, edited via one action-dispatched tool (`memory` with `action=save|search|list|get|edit|delete`) implemented in `agent/src/core/tools/memory_tools.rs`. Caps: 64 KB total, 200 entries — enforced loudly (errors propagate to the LLM). Nothing is auto-injected into the system prompt; the agent fetches on demand. Each save/edit/delete returns a capacity footer; at >=70% the agent surfaces this to the user and proposes a compaction plan rather than grooming silently. Vectors / embeddings are intentionally not used — see commit history if revisiting.

## Shared Concepts

### Config Format

```json
{
  "providers": {
    "default": "google",
    "google": {
      "api_key": "...",
      "model": "gemini-2.5-flash",
      "base_url": "https://generativelanguage.googleapis.com/v1beta/openai"
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

**ESP32 talks one wire format: OpenAI-compatible.** Every provider — Gemini, OpenAI, zAI, Anthropic, etc. — exposes `/chat/completions` with `Bearer <key>` auth. Gemini routes through the OpenAI-compat endpoint at `…/v1beta/openai/chat/completions`. The runner auto-appends `/openai` to legacy `…/v1beta` configs so existing devices migrate without manual edits. Per-provider quirks (e.g. Gemini's `extra_content.google.thought_signature`) are carried opaquely on `ToolCall.extra_content` and round-tripped verbatim — they persist through JSONL session files automatically.

The desktop runner (`core/runner.rs`) uses the `genai` crate with native wire formats and is independent. The deletion of ESP32's hand-rolled Gemini wire format (commit history pre-`OpenAI-compat pivot`) means ESP32 is no longer affected by Gemini-native protocol changes (`thoughtSignature`, parts shape, `systemInstruction` semantics).

### NVS Storage

WiFi credentials use the `wifi` NVS namespace with keys `ssid` and `password`. Config uses the `config` namespace with key `json`. NVS data survives firmware reflash and filesystem format.

### Telegram

Long-polling for inbound messages, Bot API for sends. Config requires `channels.telegram.enabled: true`, `bot_token`, and `default_chat_id`. Optional: `allowed_chat_ids` list, `stream_debounce_ms`.

### Session System

Each `chat_id` gets a JSONL file at `data/sessions/{chat_id}.jsonl` (or `/data/sessions/` on ESP32). Branching conversation trees with compaction summaries.

### Memory Considerations

All supported boards ship PSRAM: DevKitC has 8MB, Guition P4 has 32MB. The internal SRAM is reserved for hot/IRQ-safe data; tool results, large strings, and the message queue land in PSRAM heap. TLS alone needs ~40-50KB; per-tool result budgets sit comfortably in the MB range. Session compaction keeps JSONL files bounded on flash. (Boards without PSRAM are not supported.)

## Project Structure

```
zenclaw/
  agent/                    Rust agent (ESP32-S3 + ESP32-P4 + desktop targets)
    justfile                  Multi-board build system (just build/flash/clean/list)
    boards/                   Per-board TOML manifests (devkitc, guition-p4)
    bootloaders/              Vendored bootloaders (esp32s3.bin, esp32p4.bin)
    scripts/board-env.sh      Reads a board manifest and exports build env vars
    Cargo.toml                Dependencies, features, ESP-IDF components
    partitions.csv            Flash partition layout (NVS + 4MB app + 8MB LittleFS)
    sdkconfig.defaults        Shared ESP-IDF config (flash size, TLS, HTTP server)
    sdkconfig.board.devkitc   DevKitC profile (PSRAM, UART console, USB Host)
    sdkconfig.board.guition-p4  Guition P4 profile (EMAC, RISC-V, 32MB PSRAM, FATFS LFN)
    bindings_usb_msc.h        Bindgen header for USB Host MSC component
    bindings_led_strip.h      Bindgen header for LED strip component
    bindings.h                Top-level bindings (esp_chip_info, FATFS+SDMMC, zenclaw_sd helper)
    src/                      Rust source (see Architecture above)
    components/               Local IDF components built unconditionally; LTO strips unused
      zenclaw_sd/             SDMMC + FATFS + on-chip LDO glue for the SD slot

  agent-smoke/              Minimal reference crate for porting to new chips

  web/                      Nuxt web UI (PWA dashboard, config editor, file manager, provisioning)
```

## Common Pitfalls (General)

- **Gemini auth (legacy)**: Gemini's *native* `/v1beta/models/{model}:generateContent` endpoint uses `?key=API_KEY` in URL and rejects Bearer. ZenClaw doesn't use it anymore on ESP32 (we route through OpenAI-compat with Bearer). Documented for reference.
- **Multi-tool-call splitting**: A single LLM response containing N `tool_calls` MUST become ONE assistant message with all N calls, followed by N `tool` results matched by `tool_call_id`. Splitting a multi-call response into N synthesized assistant turns silently breaks Gemini (signatures attached to the original turn don't propagate to splits) and corrupts most providers' history protocols. See `agent_loop.rs::execute_tool_calls`.
- **Session history poisoning**: Repeated tool failures in history can cause the LLM to hallucinate the same error. Clear the session file to reset.
- **ESP32 NVS**: Never `espflash erase-flash` — wipes WiFi creds and config. Flash specific offsets only.
