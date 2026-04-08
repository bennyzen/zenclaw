# ZenClaw Web UI — Design Spec

## Overview

A browser-based PWA for managing ESP32 devices running ZenClaw. Hosted on GitHub Pages (static), communicates with the ESP32 over USB (Web Serial) for provisioning and HTTPS/WebSocket for daily management. All heavy UI logic runs in the browser; the ESP32 only serves a thin API.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  Browser (Nuxt 4 PWA)                            │
│                                                  │
│  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ Web      │  │ File     │  │ Provisioning  │  │
│  │ Serial   │  │ Manager  │  │ Wizard        │  │
│  │ (USB)    │  │          │  │               │  │
│  └────┬─────┘  └────┬─────┘  └──────┬────────┘  │
│       │              │               │           │
│  ┌────┴──────────────┴───────────────┴────────┐  │
│  │  Connection Layer                          │  │
│  │  ├─ SerialTransport (Web Serial API)       │  │
│  │  └─ NetworkTransport (HTTPS + WSS)         │  │
│  └───────────────┬────────────────────────────┘  │
└──────────────────┼───────────────────────────────┘
                   │
        ┌──────────┴──────────┐
        │  USB    │  Network  │
        ▼         ▼
┌───────────────────────────────────┐
│  ESP32-S3                         │
│                                   │
│  ┌─────────────────────────────┐  │
│  │  Microdot API Server        │  │
│  │  (REST + WebSocket, HTTPS)  │  │
│  └────────────┬────────────────┘  │
│               │                   │
│  ┌────────────┴────────────────┐  │
│  │  Existing ZenClaw Agent    │  │
│  │  gateway / tools / sessions │  │
│  └─────────────────────────────┘  │
└───────────────────────────────────┘
```

## Repo Structure

Monorepo with firmware and web app as siblings:

```
zenclaw/
  firmware/                 ← All ESP32 code (moved from root)
    boot.py
    main.py
    run.py
    chat_test.py
    test_tools.py
    config.json
    zenclaw_paths.py
    firmware-version.json
    provision_wifi.py
    agent/                  ← Existing agent code (untouched)
    lib/
      api/                  ← NEW: Microdot API server
        server.py           Microdot app, HTTPS/SSL, CORS
        routes.py           REST + WebSocket endpoint handlers
      wifi.py
      httpclient.py
      sys/
    stubs/
    data/

  web/                      ← NEW: Nuxt 4 app
    nuxt.config.ts
    app.vue
    pages/
      index.vue             Dashboard
      provision.vue         Provisioning wizard
      files.vue             File manager
      config.vue            Config editor (form-based)
      wifi.vue              WiFi settings
    components/
    composables/
      useConnection.ts      Connection layer (Serial + Network)
      useDevice.ts          Device state, stats
    public/
      firmware/             Pre-built firmware + filesystem images
        micropython.bin
        zenclaw.img
    package.json
```

## Tech Stack

- **Frontend**: Nuxt 4, Nuxt UI 4, TypeScript
- **ESP32 API**: Microdot (MicroPython web framework), HTTPS with self-signed cert
- **Flashing**: esptool-js (Web Serial API)
- **Hosting**: GitHub Pages (static generation via `nuxt generate`)
- **Build**: `mklittlefs` to package firmware directory into flashable filesystem image

## Connection Layer

A unified connection interface that abstracts the two transports. The rest of the app calls methods like `listDir()`, `readFile()`, `getStatus()` without knowing which transport is active.

### Serial Transport (USB)

- Uses the Web Serial API
- For: flashing firmware, provisioning WiFi/API keys on a fresh device
- Protocol: newline-delimited JSON-RPC over serial
- Available when: USB cable connected, user grants browser serial permission

### Network Transport (HTTPS + WSS)

- Uses `fetch()` for REST, native `WebSocket` for live data
- For: file management, config editing, system stats
- Available when: ESP32 is on the network, user has accepted the self-signed cert
- ESP32 IP is discovered during provisioning or entered manually

### Connection State Machine

```
Disconnected → Serial Connected → (flash/provision) → Network Connected
                                                          ↕
                                              Serial + Network (both)
```

Connection status is visible in the persistent footer.

## ESP32 API Server

Microdot runs as an async task alongside Telegram poller and heartbeat. Thin wrappers around existing code.

### REST Endpoints

| Method | Path | Purpose | Maps to |
|--------|------|---------|---------|
| `GET` | `/api/status` | System stats (RAM, uptime, temp, version) | `board.get_info()`, `gc.mem_free()` |
| `GET` | `/api/config` | Read config (API keys redacted) | `config.json` read |
| `PUT` | `/api/config` | Update config | `config.json` write |
| `GET` | `/api/files?path=` | List directory | `os.listdir()` |
| `GET` | `/api/files/read?path=` | Read file content | file read |
| `PUT` | `/api/files/write` | Write/create file | file write |
| `DELETE` | `/api/files?path=` | Delete file/directory | `os.remove()` / `os.rmdir()` |
| `POST` | `/api/files/upload` | Upload file (binary) | file write |
| `GET` | `/api/wifi` | WiFi status + stored SSID | `wifi.is_connected()`, `wifi.get_credentials()` |
| `PUT` | `/api/wifi` | Set WiFi credentials + reconnect | `wifi.set_credentials()`, `wifi.connect()` |
| `POST` | `/api/restart` | Reboot device | `machine.reset()` |

### WebSocket Endpoints

| Path | Purpose | Push interval |
|------|---------|---------------|
| `/ws/stats` | Live system stats (RAM, temp, uptime, RSSI) | Every 3 seconds |
| `/ws/logs` | Live log stream | Real-time |

### Implementation

- `firmware/lib/api/server.py` — Microdot app setup, HTTPS/SSL config, CORS headers for the hosted webapp origin
- `firmware/lib/api/routes.py` — Endpoint handlers, each a thin wrapper around existing functions
- Self-signed cert generated on first boot, stored in flash (`data/certs/`)
- Server started in `main.py` as another `bg_tasks.start()` call
- Estimated size: ~200 lines total

### CORS

The API must allow requests from the GitHub Pages origin (e.g., `https://username.github.io`). Microdot supports CORS via the `microdot-cors` plugin. Headers:
- `Access-Control-Allow-Origin: <webapp origin>`
- `Access-Control-Allow-Methods: GET, PUT, POST, DELETE`
- `Access-Control-Allow-Headers: Content-Type`

## Provisioning Wizard

Step-by-step flow for setting up a blank ESP32 from the browser.

### Steps

1. **Connect USB** — user clicks "Connect Device", browser prompts for serial port selection
2. **Detect device** — esptool-js reads chip info (ESP32-S3, flash size, MAC address). Displayed to user for confirmation.
3. **Flash** — webapp fetches `micropython.bin` + `zenclaw.img` from `web/public/firmware/`, flashes both to the appropriate partition offsets via esptool-js. Single progress bar for the whole operation.
4. **Configure WiFi** — user enters SSID and password. Sent over serial, stored in NVS via `provision_wifi.py` logic.
5. **Configure API key** — user enters their LLM provider API key. Written to `config.json` on the device over serial.
6. **Test connection** — device connects to WiFi. Webapp gets the device's IP over serial, switches to network mode, pings `GET /api/status` to confirm HTTPS works.
7. **Done** — device is running, webapp is connected over the network. User is redirected to the dashboard.

### Firmware Images

Pre-built and stored in `web/public/firmware/`:
- `micropython.bin` — standard MicroPython firmware for ESP32-S3
- `zenclaw.img` — littlefs2 filesystem image containing all ZenClaw files

Build step (CI or manual): `mklittlefs -c firmware/ -s <partition_size> web/public/firmware/zenclaw.img`

### SSL Certificate Generation

On first boot after provisioning, the device generates a self-signed SSL certificate and stores it in `data/certs/`. The microdot server uses this cert for HTTPS. The user accepts the cert once in the browser by visiting `https://<esp-ip>:<port>/` directly.

## Pages

### Dashboard (`/`)

- Device connection status (prominent)
- Quick stats: RAM, flash, temp, uptime
- Quick actions: restart device, open file manager, open config
- Device info: chip type, MAC address, firmware version, ZenClaw version

### Provisioning Wizard (`/provision`)

- Step-by-step wizard UI (see Provisioning Wizard section)
- Progress indicators for flash operations
- Form inputs for WiFi and API key configuration

### File Manager (`/files`)

- Directory tree (left panel)
- File viewer/editor (right panel)
- Syntax highlighting for `.py`, `.json`, `.md` files
- Upload/download files
- Create/rename/delete files and directories
- Binary files show download button (no inline rendering)
- This is the power-user interface — no hand-holding, full filesystem access

### Config Editor (`/config`)

- Form-based UI for editing `config.json`
- Fields for: provider selection, API keys, model name, agent name, heartbeat toggle, channel settings
- Validation before save (prevents broken config)
- This is the primary way users configure their device

### WiFi Settings (`/wifi`)

- Current connection status (connected/disconnected, IP, RSSI)
- SSID and password fields
- Save + reconnect button
- Works over both serial (during provisioning) and network (after setup)

## Persistent Footer

Visible on all pages. Shows live device stats via `/ws/stats` WebSocket.

Contents:
- Connection mode indicator (USB / Network / Disconnected)
- ESP32 IP address (when network connected)
- RAM usage (used KB / total KB)
- Flash free space
- CPU temperature
- Uptime
- WiFi signal strength (RSSI)

When disconnected: last known values shown grayed out.

## Authentication

None for v1. Local network trust model. Can be added later (API key or shared secret).

## Not In Scope (v1)

- Chat interface (Telegram covers this)
- Memory management UI
- Cron job management UI
- Detailed stats graphs / history
- OTA firmware updates over network
- Multi-device management

## Build & Deploy

### Web App

```bash
cd web
npm install
npm run generate          # Static output to web/.output/public/
# Deploy to GitHub Pages (via CI or manual push)
```

### Firmware Filesystem Image

```bash
mklittlefs -c firmware/ -s <partition_size> web/public/firmware/zenclaw.img
```

### ESP32 Manual Deploy (existing flow, paths updated)

```bash
cd firmware
mpremote cp -r agent/ :agent/
mpremote cp -r lib/ :lib/
mpremote cp -r data/ :data/
mpremote cp -r stubs/ :stubs/
mpremote cp boot.py main.py config.json zenclaw_paths.py firmware-version.json :
mpremote reset
```

## Migration

Before any new code is written, the existing codebase is restructured:

1. Create `firmware/` directory
2. Move all ESP32 code (agent/, lib/, stubs/, data/, boot.py, main.py, run.py, chat_test.py, test_tools.py, config.json, zenclaw_paths.py, firmware-version.json, provision_wifi.py) into `firmware/`
3. Update CLAUDE.md paths and commands
4. Update README.md paths and commands
5. Single commit with clear message

This is done as a standalone commit before any web UI work begins.
