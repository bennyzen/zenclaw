# Web Help System Design

**Date:** 2026-04-08
**Status:** Approved

## Problem

Users landing on `bennyzen.github.io/zenclaw/` see an empty dashboard with no context. There's no guidance on what ZenClaw is, what hardware to buy, how to provision a device, or how to configure features like Telegram and cloud storage. The target audience is curious developers who may not know hardware well.

## Design

Two features: a **welcome landing** for first-time visitors and a **contextual help drawer** accessible from any page.

### 1. Welcome Landing (empty dashboard state)

When no device is connected and the user is on `/`, replace the empty dashboard with a landing page:

**Hero section:**
- Title: "ZenClaw" with tagline: "AI agent on a $3 microcontroller"
- 2-3 sentence description of what it is and what the web UI does

**Two action cards (side by side):**
1. **Provision a new device** — description + "Get started" button → `/provision`
2. **Connect to existing device** — description + hostname input + Connect button (reuses ConnectionBanner logic)

**Hardware info section:**
- What to buy: ESP32-S3 dev board with USB (~$3-5)
- What you need: WiFi network, LLM API key (Google Gemini has a free tier)
- Browser requirement: Chrome or Edge (Web Serial API)

### 2. Help Drawer (contextual slide-over)

A `?` icon in the header opens a `USlideover` from the right. Content is route-specific:

| Route | Help content |
|---|---|
| `/` (dashboard, connected) | Dashboard stats explained (memory, storage, cloud, temperature, uptime, WiFi). How to use quick actions. |
| `/provision` | Hardware requirements (ESP32-S3 with USB). Step-by-step: configure WiFi + API key → flash via USB → wait for device. Troubleshooting: bootloader mode (hold BOOT + press RESET), Linux permissions (`dialout` group), port selection. |
| `/config` | Provider setup: pick Google Gemini or OpenAI-compatible, enter API key, choose model. Cloud storage: why it matters (flash wear), Cloudflare R2 free tier setup (create bucket, generate API token), Backblaze B2 alternative. |
| `/config` (Telegram) | Step-by-step BotFather flow: `/newbot` → name → get token. Start conversation with bot. Get chat ID via `https://api.telegram.org/bot<TOKEN>/getUpdates`. Paste token + chat ID into Config. For groups: add bot to group, set `allowed_chat_ids`. DMs get direct replies; groups get edit-based streaming. |
| `/chat` | How to chat with the agent. Available tools overview (file I/O, code exec, memory, web search, cron, etc.). How sessions work. Slash commands (`/new`, `/reset`). |
| `/files` | Local file browser: read/write/edit device files. Cloud file browser: presigned URL uploads/downloads. The `sys/` prefix for agent data. |
| `/logs` | Serial monitor output. What log levels mean. How to read boot sequence. |

### 3. File structure

```
web/app/
  components/
    WelcomeLanding.vue          Hero + action cards (empty dashboard state)
    HelpDrawer.vue              USlideover wrapper, route-aware content switching
    help/
      HelpDashboard.vue         Dashboard stats explanation
      HelpProvision.vue         Hardware + flashing guide
      HelpConfig.vue            Provider + cloud storage setup
      HelpTelegram.vue          Telegram bot setup guide
      HelpChat.vue              Chat + tools overview
      HelpFiles.vue             File manager guide
      HelpLogs.vue              Logs explanation
```

Modified files:
- `app.vue` — add `?` help icon in header, include `<HelpDrawer />`
- `pages/index.vue` — show `<WelcomeLanding />` when disconnected, current dashboard when connected

### 4. Technical notes

- No new dependencies. Uses Nuxt UI's `USlideover`, `UButton`, `UIcon`, `UCard`
- Help content is static Vue components (headings, paragraphs, `UCallout` for tips/warnings)
- Route detection via `useRoute().path` in `HelpDrawer.vue`
- The help drawer icon goes in the header next to the color mode button
- Welcome landing replaces `index.vue` content only when `state.networkConnected === false`
