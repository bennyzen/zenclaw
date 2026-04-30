<p align="center">
  <img src="zenclaw.webp" alt="ZenClaw running on an ESP32 board">
</p>

# ZenClaw

A fully autonomous AI agent that runs on a $3 ESP32 microcontroller — tool use, persistent memory, cron scheduling, multi-channel messaging, all on-device. Cloud-backed persistence (S3-compatible) protects your data from flash wear and reflashes. Works with any LLM provider: Gemini, OpenAI, DeepSeek, Groq, local models via Ollama, or anything OpenAI-compatible. Written in Rust on `esp-idf-svc`, deployable straight from the browser via Web Serial. Supports ESP32-S3 (WiFi) and ESP32-P4 (Ethernet).

## Features

- **Multi-provider LLM support**: Google Gemini (native API), any OpenAI-compatible provider (OpenAI, DeepSeek, Groq, local models via Ollama, etc.)
- **Tool-use agent loop**: Call LLM, execute tools, persist context, repeat
- **Consolidated tool system**: ~20 tools using an action-param pattern (file I/O, code exec, memory, cron, web, sub-agents, MCP client, cloud storage, skills)
- **Circuit breaker**: Detects stuck loops, no-progress polling, ping-pong patterns
- **Persistent memory**: Markdown-backed memory store with keyword search (vector embeddings deferred — see [`CLAUDE.md`](CLAUDE.md))
- **Session management**: JSONL-persisted branching conversation trees
- **Sub-agents**: Spawn isolated background agent sessions with depth limits
- **Heartbeat**: Autonomous loop with cron scheduling and reflection turns
- **Multi-channel**: Web UI, Telegram (voice, photos, typing indicator), HTTP API
- **Cloud persistence**: Write-through S3-compatible sync — sessions, memory, cron jobs, and user files automatically backed up to cloud storage (Cloudflare R2 free tier, Backblaze B2, AWS S3) and restored on boot
- **Web UI**: Nuxt PWA — dashboard, config editor, file manager, browser-based device provisioning via Web Serial
- **Multi-board**: ESP32-S3 (DevKitC) and ESP32-P4 (Guition); WiFi or Ethernet; multiple devices coexist on one network via mDNS

## Quick Start

Open [bennyzen.github.io/zenclaw](https://bennyzen.github.io/zenclaw/) in Chrome or Edge (Web Serial required). Plug your ESP32 board in via USB and go to the Provision page. The wizard handles everything:

1. **Configure** — Pick a board (DevKitC or Guition P4), enter a device name (or roll one), supply WiFi credentials (skipped for Ethernet boards), pick an LLM provider, paste your API key
2. **Flash** — The browser flashes the firmware image and an NVS partition (device hostname + WiFi creds) in one shot via Web Serial. No CLI tools, no manual file copying
3. **Connect** — The device boots, joins the network (WiFi for S3, Ethernet for P4), and appears at `<devicename>.local`. The wizard pushes the LLM provider config automatically

Done. The device is running at `http://<devicename>.local`. The dashboard connects to it from the same hosted web UI — your browser bridges to the device on your local network.

For developers building from source, see [`agent-esp32/`](agent-esp32/) and [`CLAUDE.md`](CLAUDE.md) for board manifests, build commands, and Rust architecture.

## Architecture

```
agent-esp32/src/
  main.rs          ESP32 entry: NIC bring-up, mDNS, SPIFFS, HTTP server, Telegram poller
  core/            Shared agent logic
    gateway.rs     Core orchestrator, chat() entry point
    agent_loop.rs  LLM <-> tool execution loop
    runner.rs      Provider dispatch trait
    tools/         Tool implementations (action-param pattern)
    sessions/      JSONL conversation persistence
    channels/      Channel abstraction (Telegram, API)
    memory/        Persistent memory store
    telegram.rs    Telegram bot (long-poll + send)
    cron.rs        Scheduled tasks
  net/             NIC abstraction (WiFi for S3, Ethernet for P4)
  esp32/           ESP32 HTTP runner (esp-idf-svc)
  desktop/         Desktop HTTP server + client (axum + reqwest)
```

See [`CLAUDE.md`](CLAUDE.md) for the full architecture, board profiles, and build workflow.

## Project Structure

```
zenclaw/
  agent-esp32/              Rust agent (ESP32-S3 + ESP32-P4 + desktop targets)
    boards/                 Per-board TOML manifests (devkitc, guition-p4)
    bootloaders/            Vendored ESP-IDF bootloaders
    src/                    Rust source (see CLAUDE.md)
    justfile                Multi-board build commands
  agent-esp32-smoke/        Minimal reference crate for porting to new chips
  web/                      Nuxt web UI (PWA dashboard, config editor, file manager, provisioning)
  scripts/                  Build helpers (build-rust-firmware.sh)
  docs/                     Specs, plans, design documents
```

## Configuration

Configuration is handled through the web UI — the Config page edits the device's stored config directly over your local network. The provisioning wizard sets up the initial provider and API key. Example shape:

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

Multiple providers can be configured. The `default` key selects which one to use. Provider `base_url` determines the wire format: Gemini URLs use Gemini's native format, everything else uses OpenAI-compatible format (`POST /chat/completions` with Bearer auth). Any OpenAI-compatible API (DeepSeek, Groq, Ollama, etc.) works out of the box.

## Cloud Persistence

The ESP32 has limited, wear-prone flash storage. Filesystem corruption from power loss, firmware reflashes, or flash wear is a real risk. ZenClaw mitigates this with automatic write-through replication to S3-compatible cloud storage.

**How it works:**

1. **Boot restore**: On startup, missing local files are downloaded from the bucket — sessions, memory, cron jobs, user files
2. **Background sync**: Dirty files replicate to the bucket asynchronously. Local writes happen at full speed
3. **Initial backup**: On first boot with sync configured, all existing local files are uploaded

**Supported providers**: any S3-compatible service — Cloudflare R2 (10 GB free tier), Backblaze B2, AWS S3, MinIO, etc.

**What gets synced**: sessions, memory, cron jobs, and user files. Generated binaries and images are excluded.

Configure storage via the web UI's Config page or POST to `/api/config`:

```json
{
  "storage": {
    "endpoint": "https://<account>.r2.cloudflarestorage.com",
    "access_key_id": "...",
    "secret_access_key": "...",
    "bucket": "zenclaw",
    "region": "auto"
  }
}
```

Agent system data is stored under a `sys/` prefix in the bucket (stripped transparently). User files uploaded via the file manager go to the bucket root. The web UI provides a cloud file browser with presigned URLs for direct browser-to-bucket uploads and downloads.

## Agent Identity

The agent's personality and instructions live in `SOUL.md` on the device's filesystem. Edit it via the web UI's File Manager to customize how ZenClaw behaves.

## License

MIT
