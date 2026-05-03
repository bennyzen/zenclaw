# ZenClaw Web UI

Browser-based companion app for [ZenClaw](../) devices — a PWA for provisioning new ESP32 boards over Web Serial, then managing them across the local network. Built with [Nuxt 4](https://nuxt.com), [Nuxt UI v4](https://ui.nuxt.com), and [Tailwind CSS](https://tailwindcss.com).

## What it does

- **Provision** a new ESP32-S3 or ESP32-P4 over Web Serial (Chrome / Edge desktop only — no driver install needed)
- **Connect** to any device on the LAN by mDNS hostname (`zenclaw-<name>.local`); switch between devices freely
- **Chat** with the agent in real time over WebSocket, with streaming tool-call widgets
- **Browse files** on the device's SPIFFS partition and on its bound R2 bucket
- **Edit config** (providers, API keys, channels, cloud storage) with a CodeMirror editor
- **Inspect memory**, watch live logs, restart the device

The UI talks to the device's HTTP / WebSocket API directly — there's no backend in front. State lives client-side; the dev server is just for local iteration.

## Setup

The project uses npm. The `packageManager` field in `package.json` pins the version — don't introduce other lockfiles.

```bash
npm install   # also runs `nuxt prepare` to generate .nuxt/ types
```

## Development

```bash
npm run dev   # http://localhost:3000
```

Open `http://localhost:3000`, type a device hostname (e.g. `zenclaw-swift-fox`) on the landing page, and click Connect. The hostname is persisted to `localStorage` so reloading reconnects automatically.

For provisioning a brand-new board, plug it in via USB and visit `/provision` — the wizard reads board manifests from `public/firmware/firmware.json` (built by `../scripts/build-rust-firmware.sh`).

### Browser requirements

| Feature | Requirement |
|---|---|
| Provisioning a new device | Chrome / Edge desktop (Web Serial API) |
| Dashboard, chat, config, files | Any modern browser |

### Talking to a device locally

Devices announce themselves on mDNS as `<hostname>.local`. On Linux this requires `avahi-daemon`; on macOS / Windows it works out of the box. If mDNS fails, you can also enter a raw `IP[:port]` in the connect field.

## Production build

```bash
npm run build           # SSG output in .output/public
npm run generate        # explicit static generation (same as build with ssr:false)
npm run preview         # serve the built bundle locally
```

The app is configured with `ssr: false` and prerenders to a static bundle — host it anywhere (Cloudflare Pages, S3, GitHub Pages). Set `NUXT_APP_BASE_URL` at build time if serving from a subpath.

## Project layout

```
app/
  app.vue                    Root: header nav, ConnectionBanner, NuxtPage, footer
  pages/
    index.vue                Landing — provision card + connect/disconnect card
    dashboard.vue            Connected device overview (memory, storage, cloud, embedded device page)
    chat.vue                 Streaming chat with tool-call widgets
    provision.vue            Web Serial flashing wizard
    config.vue               JSON config editor (CodeMirror)
    files.vue                Device SPIFFS + cloud R2 file browser
    memory.vue               Agent memory viewer
    logs.vue                 Live log stream
  components/
    ConnectionBanner.vue     Top-bar connect prompt (hidden when connected)
    WelcomeLanding.vue       Landing page hero + cards
    AppFooter.vue            Status footer (live device stats)
    HelpDrawer.vue + help/   Per-page contextual help
  composables/
    useConnection.ts         Module-scoped reactive state — single source of truth for the active device
    useSerial.ts             Web Serial wrapper used by the provisioning wizard
  types/connection.ts        Shared types for events, device status, file entries
```

`useConnection` exposes one shared `state` object plus action functions (`connectNetwork`, `disconnectNetwork`, file ops, chat ops, etc.). Pages read `state` reactively — there's no Pinia / Vuex.

## Useful pointers

- **Component theming**: per Nuxt UI v4, slot-level overrides go through `:ui="{ slot: 'classes' }"` on each component. Generated theme files live in `.nuxt/ui/<component>.ts` after `nuxt prepare` runs.
- **Markdown rendering**: chat assistant messages use `@nuxtjs/mdc` with `remark-emoji`; configured in `nuxt.config.ts` and `app/assets/css/main.css`.
- **PWA manifest**: `public/manifest.json` + theme color in `nuxt.config.ts`. There's no service worker yet.
- **Vite optimizeDeps**: `nuxt.config.ts` pre-bundles a few heavy deps (CodeMirror language modes, esptool-js, etc.) to avoid first-load reload churn — add to that list if Vite warns about late-discovered deps.

## Architecture notes

For deeper context — including the device API surface, channel system, and cross-cutting design decisions — see the project-wide [`CLAUDE.md`](../CLAUDE.md) one level up.
