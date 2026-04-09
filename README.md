<p align="center">
  <img src="zenclaw.webp" alt="ZenClaw running on an ESP32-S3 board">
</p>

# ZenClaw

A fully autonomous AI agent that fits on a $3 ESP32-S3 microcontroller — 512KB of SRAM, cloud-backed persistence (S3-compatible), 40+ built-in tools, vector memory, cron scheduling, and a Telegram bot. Works with any LLM provider: Gemini, OpenAI, DeepSeek, Groq, local models via Ollama, or anything OpenAI-compatible. Built for MicroPython and deployable straight from the browser via Web Serial.

## Features

- **Multi-provider LLM support**: Google Gemini (native API), any OpenAI-compatible provider (OpenAI, DeepSeek, Groq, local models via Ollama, etc.)
- **Tool-use agent loop**: Call LLM, execute tools, persist context, repeat
- **40+ built-in tools**: File I/O, code exec, vector memory, cron scheduling, web search, sub-agents, MCP client, image generation, Google Sheets, cloud storage (S3), Telegram messaging
- **Circuit breaker**: Detects stuck loops, no-progress polling, ping-pong patterns
- **Vector memory**: Keyword + embedding hybrid search with persistent markdown storage
- **Session management**: JSONL-persisted branching conversation trees
- **Sub-agents**: Spawn isolated background agent sessions with depth limits
- **Heartbeat**: Autonomous loop with cron scheduling and reflection turns
- **Multi-channel**: CLI and Telegram (voice, photos, typing indicator)
- **Cloud persistence**: Write-through S3-compatible sync — the device is brittle (flash wear, filesystem corruption, firmware reflashes), so sessions, memory, cron jobs, and user files are automatically backed up to cloud storage (Cloudflare R2 free tier, Backblaze B2, AWS S3) and restored on boot
- **Web UI**: Nuxt PWA — dashboard, config editor, file manager, ESP32 provisioning via Web Serial
- **ESP32-S3 ready**: WiFi via NVS, headless boot, hardware detection, SD card support

## Quick Start

### ESP32-S3 (browser provisioning)

Open [bennyzen.github.io/zenclaw](https://bennyzen.github.io/zenclaw/) in Chrome or Edge (Web Serial required). Plug your ESP32-S3 via USB and go to the Provision page. The wizard handles everything:

1. **Configure** — Enter WiFi credentials, pick an LLM provider, enter your API key, choose a device name
2. **Flash** — The browser flashes MicroPython + LittleFS filesystem + NVS (WiFi creds) in one shot via Web Serial. No CLI tools, no manual file copying
3. **Connect** — The device boots, joins your WiFi, and appears at `devicename.local`. The wizard pushes the API key config automatically

Done. The device is running at `http://devicename.local`. The dashboard connects to it from the same hosted web UI — your browser bridges to the device on your local network.

### Desktop (MicroPython unix port)

```bash
cp firmware/config.example.json firmware/config.json
# Edit firmware/config.json with your provider API key, then:
cd firmware && micropython -X heapsize=4m run.py
```

### Testing

```bash
# Smoke test all tools (no LLM needed)
cd firmware && micropython -X heapsize=4m test_tools.py

# Send a single message through the full LLM pipeline
cd firmware && micropython -X heapsize=4m chat_test.py "What time is it?"

# Fresh session, quiet output
cd firmware && micropython -X heapsize=4m chat_test.py --reset --quiet "List my files"
```

## Architecture

```
firmware/boot.py (ESP32)                   WiFi from NVS -> connect
firmware/main.py (ESP32) / firmware/run.py (desktop)
  gateway.py                    — Core orchestrator, config, lifecycle
    prompt.py                   — System prompt from SOUL.md + tools + skills
    agent_loop.py               — LLM -> tool execution -> repeat
      runner.py                 — Provider dispatch, retry, streaming
      providers/                — Gemini native API + OpenAI-compatible format
      tools/                    — 40+ registered tools
      tool_loop.py              — Circuit breaker for stuck loops
    session_manager/            — JSONL branching conversation trees
    heartbeat_runner.py         — Autonomous background loop + cron
    telegram/                   — Polling, sending, media, typing indicator
    channels/                   — CLI and Telegram delivery
```

## Project Structure

```
zenclaw/
  firmware/                 ESP32 firmware (MicroPython agent)
    boot.py                 ESP32 boot (WiFi from NVS)
    main.py                 ESP32 entry (headless, Telegram)
    run.py                  Desktop entry (interactive REPL)
    provision_wifi.py       WiFi credential provisioning
    chat_test.py            Programmatic LLM test harness
    test_tools.py           Tool smoke tests
    config.example.json     Config template (copy to config.json with your keys)
    zenclaw_paths.py        Data directory path definitions

    agent/                  Main agent package
      gateway.py            Orchestrator + ZenClawGateway class
      agent_loop.py         Core LLM <-> tool loop
      runner.py             Provider dispatch + retry
      prompt.py             System prompt builder
      providers/            LLM API implementations
      tools/                40+ tool modules
      session_manager/      Conversation persistence
      telegram/             Bot polling, sending, media
      channels/             Channel abstraction (cli, telegram)
      cron/                 Scheduled task execution
      subagents/            Background agent spawning

    lib/                    MicroPython support libraries
      wifi.py               WiFi + NVS credential management
      httpclient.py         HTTP client (get, post, stream)
      sys/                  Logging, background tasks, board detection

    stubs/                  Desktop compatibility stubs
    data/                   Runtime data (sessions, memory, cron)

  web/                      Nuxt web UI (PWA dashboard, config, files, provisioning)
```

## Configuration

For ESP32, configuration is handled through the hosted web UI — the Config page edits `config.json` on the device directly over your local network. The provisioning wizard sets up the initial provider and API key.

For desktop, copy `firmware/config.example.json` to `firmware/config.json` and edit it. Example:

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

Multiple providers can be configured. The `default` key selects which one to use. Provider `base_url` determines the wire format: Gemini URLs use Gemini native format, everything else uses OpenAI-compatible format (`POST /chat/completions` with Bearer auth). This means any OpenAI-compatible API (DeepSeek, Groq, Ollama, etc.) works out of the box.

## Cloud Persistence

The ESP32 is a $3 microcontroller with limited, wear-prone flash storage. Filesystem corruption from power loss, firmware reflashes, or flash wear is a real risk. ZenClaw mitigates this with automatic write-through replication to S3-compatible cloud storage.

**How it works:**

1. **Boot restore**: On startup, `pull_from_cloud()` downloads any local files missing from `data/` — sessions, memory, cron jobs, user files
2. **Background sync**: A worker uploads dirty files every 30 seconds. Local writes happen at full speed; replication is asynchronous
3. **Initial backup**: On first boot with sync configured, all existing local files are uploaded to the cloud bucket

**Supported providers**: Any S3-compatible service — Cloudflare R2 (10 GB free tier), Backblaze B2, AWS S3, MinIO, etc.

**What gets synced**: Sessions (`data/sessions/`), vector memory (`data/memory/`), cron jobs (`data/cron/`), and user files. Binary files (`.bin`, `.pyc`) and generated images are excluded. Files over 512 KB are skipped to conserve memory.

**Config** (add to `config.json`):

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

Agent system data is stored under a `sys/` prefix in the bucket (stripped transparently). User files uploaded via the file manager or `storage_write` tool go to the bucket root. The web UI provides a cloud file browser with presigned URLs for direct browser-to-bucket uploads and downloads.

## Agent Identity

The agent's personality and instructions live in `firmware/data/SOUL.md`. Edit this file to customize how ZenClaw behaves.

## License

MIT
