# Web Help System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a welcome landing page for disconnected users and a contextual help drawer accessible from every page.

**Architecture:** Two components: `WelcomeLanding.vue` replaces the empty dashboard when no device is connected, and `HelpDrawer.vue` provides route-specific help via a `USlideover`. Help content lives in individual Vue components under `components/help/`.

**Tech Stack:** Nuxt UI v4 (`USlideover`, `UCard`, `UButton`, `UIcon`, `UCallout`), Vue 3 composition API, no new dependencies.

---

### Task 1: HelpDrawer component

**Files:**
- Create: `web/app/components/HelpDrawer.vue`

**Step 1: Create the HelpDrawer wrapper**

This component renders a `?` icon button (used in `app.vue` header) and a `USlideover` that shows route-specific help content. It watches the current route to pick the right help component.

```vue
<script setup lang="ts">
const open = ref(false)
const route = useRoute()

const helpComponents: Record<string, string> = {
  '/': 'HelpDashboard',
  '/provision': 'HelpProvision',
  '/config': 'HelpConfig',
  '/chat': 'HelpChat',
  '/files': 'HelpFiles',
  '/logs': 'HelpLogs',
}

const currentHelp = computed(() => helpComponents[route.path] ?? 'HelpDashboard')

function toggle() {
  open.value = !open.value
}

defineExpose({ toggle })
</script>

<template>
  <UButton
    icon="i-lucide-circle-help"
    size="sm"
    variant="ghost"
    color="neutral"
    aria-label="Help"
    @click="open = true"
  />
  <USlideover v-model:open="open" side="right" :ui="{ width: 'w-[480px]' }">
    <div class="p-6 overflow-y-auto h-full">
      <div class="flex items-center justify-between mb-6">
        <h2 class="text-lg font-semibold">Help</h2>
        <UButton icon="i-lucide-x" size="sm" variant="ghost" color="neutral" @click="open = false" />
      </div>
      <component :is="resolveComponent(currentHelp)" />
    </div>
  </USlideover>
</template>
```

**Note:** `resolveComponent` is a Nuxt auto-import that resolves a component by name. The `components/help/Help*.vue` files will be auto-registered by Nuxt's component scanning.

**Step 2: Commit**

```bash
git add web/app/components/HelpDrawer.vue
git commit -m "Add HelpDrawer component with route-aware slideover"
```

---

### Task 2: Help content components

**Files:**
- Create: `web/app/components/help/HelpDashboard.vue`
- Create: `web/app/components/help/HelpProvision.vue`
- Create: `web/app/components/help/HelpConfig.vue`
- Create: `web/app/components/help/HelpTelegram.vue`
- Create: `web/app/components/help/HelpChat.vue`
- Create: `web/app/components/help/HelpFiles.vue`
- Create: `web/app/components/help/HelpLogs.vue`

**Step 1: Create all help content components**

Each is a simple Vue file with static content — headings, paragraphs, and `UCallout` for tips/warnings.

`HelpDashboard.vue`:
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Dashboard</h3>
    <p>The dashboard shows real-time stats from your ZenClaw device.</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Device</h4>
        <p class="text-muted">Agent name and firmware version.</p>
      </div>
      <div>
        <h4 class="font-medium">Memory</h4>
        <p class="text-muted">RAM usage (SRAM + PSRAM). Normal range is 40-70%. If it's consistently above 90%, the device may need a restart.</p>
      </div>
      <div>
        <h4 class="font-medium">Device Storage</h4>
        <p class="text-muted">On-board flash filesystem. Stores sessions, memory, cron jobs, skills, and user files.</p>
      </div>
      <div>
        <h4 class="font-medium">Cloud Storage</h4>
        <p class="text-muted">Off-device backup via S3-compatible storage (Cloudflare R2, Backblaze B2, etc.). Protects against flash corruption. Configure in <NuxtLink to="/config" class="text-primary underline">Config</NuxtLink>.</p>
      </div>
    </div>

    <UCallout icon="i-lucide-info" title="Quick Actions">
      <p><strong>File Manager</strong> — browse and edit files on the device.</p>
      <p><strong>Config</strong> — edit device configuration (provider, API keys, channels).</p>
      <p><strong>Restart Device</strong> — reboots the ESP32. Connection will drop briefly.</p>
    </UCallout>

    <p>The status bar at the bottom shows live metrics: RAM, storage, cloud status, CPU temperature, uptime, and WiFi signal strength. Hover each for details.</p>
  </div>
</template>
```

`HelpProvision.vue`:
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Provisioning a Device</h3>
    <p>Set up a new ZenClaw device in three steps. You need an ESP32-S3 board with USB connected to this computer.</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">1. Configure</h4>
        <p class="text-muted">Enter your WiFi credentials (the device needs WiFi to work), pick an LLM provider, and enter your API key. Google Gemini has a <a href="https://aistudio.google.com/apikey" target="_blank" class="text-primary underline">free tier</a>.</p>
        <p class="text-muted">Choose a device name — it becomes the hostname (e.g. <code>zenclaw-bold-fox.local</code>).</p>
      </div>
      <div>
        <h4 class="font-medium">2. Flash</h4>
        <p class="text-muted">Plug your ESP32-S3 via USB and click Flash. The browser flashes MicroPython + the ZenClaw filesystem + WiFi credentials in one operation via Web Serial. No CLI tools needed.</p>
      </div>
      <div>
        <h4 class="font-medium">3. Connect</h4>
        <p class="text-muted">After flashing, the device reboots, connects to your WiFi, and appears at <code>devicename.local</code>. The wizard pushes your API key config automatically.</p>
      </div>
    </div>

    <UCallout icon="i-lucide-cpu" title="Hardware">
      <p>Any ESP32-S3 board with USB will work. Look for ones with native USB (not USB-to-TTL). Common options: ESP32-S3-WROOM dev boards, Adafruit Feather ESP32-S3, Unexpected Maker boards. Price: $3-8.</p>
    </UCallout>

    <UCallout icon="i-lucide-alert-triangle" color="warning" title="Troubleshooting">
      <p><strong>Blank device / first time:</strong> Hold BOOT, press and release RESET, release BOOT. Select "USB JTAG/serial debug unit" in the port picker.</p>
      <p><strong>Linux permissions:</strong> If you get a permissions error, add yourself to the serial group and re-login:</p>
      <code class="text-xs">sudo usermod -aG dialout $USER</code>
      <p><strong>Device not found after flash:</strong> Press RESET on the board. Wait 10-15 seconds for WiFi to connect.</p>
    </UCallout>

    <UCallout icon="i-lucide-globe" title="Browser requirement">
      <p>Web Serial API is required for flashing. Use <strong>Chrome</strong> or <strong>Edge</strong>. Firefox and Safari do not support Web Serial.</p>
    </UCallout>
  </div>
</template>
```

`HelpConfig.vue`:
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Configuration</h3>
    <p>Edit your device's configuration remotely. Changes are saved to <code>config.json</code> on the device and take effect after a restart.</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">LLM Provider</h4>
        <p class="text-muted">Choose between Google Gemini (native API) or any OpenAI-compatible provider. Enter your API key and pick a model.</p>
        <p class="text-muted">You can use the "Browse models" option to pick from OpenRouter's catalog, which gives access to hundreds of models with a single API key.</p>
      </div>
      <div>
        <h4 class="font-medium">Cloud Storage</h4>
        <p class="text-muted">The ESP32 has limited, wear-prone flash storage. Cloud storage backs up your sessions, memory, cron jobs, and user files to an S3-compatible bucket. If the device's filesystem gets corrupted, data is restored from the cloud on next boot.</p>
        <p class="text-muted"><strong>Cloudflare R2</strong> is recommended — 10 GB free, no egress fees. Create a bucket, generate an API token with Object Read/Write permissions, and paste the credentials here.</p>
      </div>
    </div>

    <UCallout icon="i-lucide-message-circle" title="Telegram">
      <p>See the dedicated <NuxtLink to="/config" class="text-primary underline">Telegram setup guide</NuxtLink> below for step-by-step instructions.</p>
    </UCallout>
  </div>
</template>
```

`HelpTelegram.vue` (linked from HelpConfig):
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Telegram Bot Setup</h3>
    <p>ZenClaw can receive and respond to messages via Telegram. Set up a bot in a few steps:</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">1. Create a bot</h4>
        <p class="text-muted">Open Telegram and message <a href="https://t.me/BotFather" target="_blank" class="text-primary underline">@BotFather</a>. Send <code>/newbot</code>, pick a display name and username. BotFather gives you a bot token (looks like <code>123456:ABC-DEF...</code>).</p>
      </div>
      <div>
        <h4 class="font-medium">2. Start a conversation</h4>
        <p class="text-muted">Send any message to your new bot in a private chat. This creates the chat history so the bot can find your chat ID.</p>
      </div>
      <div>
        <h4 class="font-medium">3. Get your chat ID</h4>
        <p class="text-muted">Visit this URL in your browser (replace <code>TOKEN</code> with your bot token):</p>
        <code class="text-xs break-all">https://api.telegram.org/botTOKEN/getUpdates</code>
        <p class="text-muted mt-1">Look for <code>"chat":{"id": 123456789}</code> in the JSON response. That number is your chat ID.</p>
      </div>
      <div>
        <h4 class="font-medium">4. Configure ZenClaw</h4>
        <p class="text-muted">In the Config page, enable Telegram, paste the bot token and your chat ID as the <code>default_chat_id</code>.</p>
      </div>
    </div>

    <UCallout icon="i-lucide-users" title="Group chats">
      <p>To use the bot in a group:</p>
      <ol class="list-decimal list-inside space-y-1 mt-1">
        <li>Add the bot to the group</li>
        <li>Send a message in the group (so it shows up in getUpdates)</li>
        <li>Get the group chat ID from getUpdates (negative number)</li>
        <li>Add it to <code>allowed_chat_ids</code> in Config</li>
      </ol>
      <p class="mt-1">In DMs the bot replies directly. In groups it uses edit-based streaming (updates the message as the response comes in).</p>
    </UCallout>

    <UCallout icon="i-lucide-info" title="Privacy">
      <p><code>allowed_chat_ids</code> restricts who can talk to the bot. If not set, only <code>default_chat_id</code> is allowed. Leave empty to allow DMs only.</p>
    </UCallout>
  </div>
</template>
```

`HelpChat.vue`:
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Chat</h3>
    <p>Talk to your ZenClaw agent through this web interface. Messages go to the LLM and responses stream back in real time.</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">How it works</h4>
        <p class="text-muted">Type a message and press Enter. The agent processes it, optionally calls tools (file operations, web search, code execution, etc.), and responds with text. The full conversation is persisted on the device.</p>
      </div>
      <div>
        <h4 class="font-medium">Available tools</h4>
        <p class="text-muted">The agent has 40+ built-in tools: file read/write/edit, code execution, vector memory, cron scheduling, web search, cloud storage, sub-agents, MCP client, image generation, Google Sheets, and more. It decides which tools to use based on your message.</p>
      </div>
      <div>
        <h4 class="font-medium">Sessions</h4>
        <p class="text-muted">Each channel (web, Telegram) has its own conversation session. Sessions are persisted as JSONL files on the device and backed up to cloud storage if configured.</p>
      </div>
    </div>

    <UCallout icon="i-lucide-terminal" title="Slash commands">
      <p>Type these in the chat input:</p>
      <ul class="list-disc list-inside space-y-0.5 mt-1">
        <li><code>/new</code> — start a fresh session</li>
        <li><code>/reset</code> — clear session history and start over</li>
      </ul>
    </UCallout>
  </div>
</template>
```

`HelpFiles.vue`:
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">File Manager</h3>
    <p>Browse and edit files on your device, or manage cloud storage files.</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Device files</h4>
        <p class="text-muted">Browse the ESP32's flash filesystem. You can read, edit, create, and delete files. The main directories:</p>
        <ul class="list-disc list-inside space-y-0.5 mt-1">
          <li><code>data/sessions/</code> — conversation histories</li>
          <li><code>data/memory/</code> — vector memory store</li>
          <li><code>data/cron/</code> — scheduled jobs</li>
          <li><code>data/skills/</code> — installed skills</li>
        </ul>
      </div>
      <div>
        <h4 class="font-medium">Cloud files</h4>
        <p class="text-muted">Browse your S3-compatible bucket. Agent system data is stored under a <code>sys/</code> prefix (stripped transparently). Files you upload go to the bucket root. Uploads and downloads use presigned URLs — data flows directly between your browser and the cloud, not through the device.</p>
      </div>
    </div>

    <UCallout icon="i-lucide-info" title="Tip">
      <p>Cloud storage requires configuration in <NuxtLink to="/config" class="text-primary underline">Config</NuxtLink>. Cloudflare R2 gives you 10 GB free.</p>
    </UCallout>
  </div>
</template>
```

`HelpLogs.vue`:
```vue
<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Logs</h3>
    <p>Live log stream from the device, sent over WebSocket. Shows agent activity, tool calls, API requests, errors, and system events.</p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Log levels</h4>
        <ul class="space-y-0.5 mt-1">
          <li><span class="text-blue-400">info</span> — normal operation (tool calls, API requests, boot sequence)</li>
          <li><span class="text-yellow-400">warning</span> — recoverable issues</li>
          <li><span class="text-red-400">error</span> — failures (API errors, network issues)</li>
          <li><span class="text-zinc-500">debug</span> — verbose diagnostic info</li>
        </ul>
      </div>
      <div>
        <h4 class="font-medium">Filtering</h4>
        <p class="text-muted">Use the level and source dropdowns to filter. The source is the module that emitted the log (e.g. <code>zenclaw</code>, <code>api</code>, <code>sync</code>).</p>
      </div>
    </div>

    <UCallout icon="i-lucide-info" title="Tip">
      <p>Logs stream in real time. If the connection drops, the page reconnects automatically. Pause the stream to scroll through history without new entries pushing you down.</p>
    </UCallout>
  </div>
</template>
```

**Step 2: Commit**

```bash
git add web/app/components/help/
git commit -m "Add help content components for all pages"
```

---

### Task 3: Wire HelpDrawer into app.vue header

**Files:**
- Modify: `web/app/app.vue`

**Step 1: Add help icon to header**

In `app.vue`, add a help button next to the color mode button in the header's `ml-auto` div. The `HelpDrawer` component handles its own open state internally.

Change the `<div class="ml-auto">` section from:
```vue
<div class="ml-auto">
  <UColorModeButton size="sm" variant="ghost" color="neutral" />
</div>
```
to:
```vue
<div class="ml-auto flex items-center gap-1">
  <HelpDrawer />
  <UColorModeButton size="sm" variant="ghost" color="neutral" />
</div>
```

**Step 2: Commit**

```bash
git add web/app/app.vue
git commit -m "Add help drawer to app header"
```

---

### Task 4: WelcomeLanding component

**Files:**
- Create: `web/app/components/WelcomeLanding.vue`

**Step 1: Create the welcome landing**

A full-page component shown when no device is connected. Includes hero, action cards, and hardware info.

```vue
<script setup lang="ts">
const { state, connectNetwork } = useConnection()

const STORAGE_KEY = 'zenclaw_provision'
const hostname = ref('')

onMounted(() => {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved) {
      const data = JSON.parse(saved)
      if (data.deviceName) hostname.value = data.deviceName
    }
  } catch { /* ignore */ }
})

async function connect() {
  if (!hostname.value) return
  try {
    await connectNetwork(hostname.value + '.local')
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || '{}')
    saved.deviceName = hostname.value
    localStorage.setItem(STORAGE_KEY, JSON.stringify(saved))
  } catch { /* error shown via state.error */ }
}
</script>

<template>
  <div class="max-w-2xl mx-auto space-y-8 py-8">
    <div class="text-center space-y-3">
      <h1 class="text-4xl font-bold">ZenClaw</h1>
      <p class="text-lg text-muted">AI agent on a $3 microcontroller</p>
      <p class="text-sm text-dimmed max-w-lg mx-auto">
        An autonomous AI agent that runs entirely on an ESP32-S3. Talk to it via Telegram or this web UI. It can read files, execute code, search the web, manage schedules, and more — all from a device that fits in your palm.
      </p>
    </div>

    <div class="grid gap-4 sm:grid-cols-2">
      <UCard>
        <div class="space-y-3">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-zap" class="text-primary text-lg" />
            <h3 class="font-semibold">Provision a new device</h3>
          </div>
          <p class="text-sm text-muted">Flash firmware and configure WiFi + API keys from your browser. Just plug in via USB.</p>
          <UButton to="/provision" label="Get started" icon="i-lucide-arrow-right" size="lg" />
        </div>
      </UCard>

      <UCard>
        <div class="space-y-3">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-plug" class="text-primary text-lg" />
            <h3 class="font-semibold">Connect to device</h3>
          </div>
          <p class="text-sm text-muted">Already have a device? Enter its hostname to connect over your local network.</p>
          <div class="flex items-center gap-2">
            <UInput
              v-model="hostname"
              placeholder="zenclaw-wild-crow"
              size="lg"
              class="flex-1"
              :disabled="state.connecting"
              @keydown.enter="connect"
            >
              <template #trailing>
                <span class="text-xs text-dimmed">.local</span>
              </template>
            </UInput>
            <UButton
              :label="state.connecting ? 'Connecting...' : 'Connect'"
              size="lg"
              :disabled="state.connecting || !hostname"
              @click="connect"
            >
              <template #leading>
                <UIcon v-if="state.connecting" name="i-lucide-loader-circle" class="animate-spin" />
                <UIcon v-else name="i-lucide-plug" />
              </template>
            </UButton>
          </div>
          <p v-if="state.error" class="text-xs text-red-400">{{ state.error }}</p>
        </div>
      </UCard>
    </div>

    <div class="space-y-3">
      <h3 class="font-semibold text-center">What you need</h3>
      <div class="grid gap-3 sm:grid-cols-3">
        <div class="text-center space-y-1 p-3">
          <UIcon name="i-lucide-cpu" class="text-primary text-xl" />
          <p class="text-sm font-medium">ESP32-S3 board</p>
          <p class="text-xs text-dimmed">Any board with USB and WiFi. $3-8 on AliExpress, Amazon, or Adafruit.</p>
        </div>
        <div class="text-center space-y-1 p-3">
          <UIcon name="i-lucide-wifi" class="text-primary text-xl" />
          <p class="text-sm font-medium">WiFi network</p>
          <p class="text-xs text-dimmed">The device needs WiFi to call LLM APIs and serve this dashboard.</p>
        </div>
        <div class="text-center space-y-1 p-3">
          <UIcon name="i-lucide-key" class="text-primary text-xl" />
          <p class="text-sm font-medium">LLM API key</p>
          <p class="text-xs text-dimmed">Google Gemini has a <a href="https://aistudio.google.com/apikey" target="_blank" class="text-primary underline">free tier</a>. Or use OpenAI, DeepSeek, Groq, etc.</p>
        </div>
      </div>
    </div>

    <UCallout icon="i-lucide-globe" title="Browser requirement">
      Provisoning requires the Web Serial API. Use <strong>Chrome</strong> or <strong>Edge</strong> to flash a device. Firefox and Safari are not supported for flashing but work for all other features.
    </UCallout>
  </div>
</template>
```

**Step 2: Commit**

```bash
git add web/app/components/WelcomeLanding.vue
git commit -m "Add WelcomeLanding component for disconnected state"
```

---

### Task 5: Wire WelcomeLanding into index.vue

**Files:**
- Modify: `web/app/pages/index.vue`

**Step 1: Show WelcomeLanding when disconnected**

The current `index.vue` only renders dashboard content when `state.networkConnected` is true. Add `WelcomeLanding` as the else branch.

Add at the top of the `<template>`, after `<h1>`:

Current template structure is:
```vue
<template>
  <div class="max-w-3xl space-y-6">
    <h1 class="text-2xl font-bold">Dashboard</h1>
    <template v-if="state.networkConnected && state.lastStatus">
      ... all dashboard content ...
    </template>
  </div>
</template>
```

Change to:
```vue
<template>
  <div v-if="!state.networkConnected">
    <WelcomeLanding />
  </div>
  <div v-else class="max-w-3xl space-y-6">
    ... all existing dashboard content ...
  </div>
</template>
```

**Step 2: Commit**

```bash
git add web/app/pages/index.vue
git commit -m "Show WelcomeLanding when no device connected"
```

---

### Task 6: Add Telegram help to Config page

**Files:**
- Modify: `web/app/components/HelpConfig.vue`

The Config help already mentions Telegram. To make the Telegram help content directly accessible, update `HelpConfig.vue` to include the full `HelpTelegram` component inline below its main content.

Add at the bottom of the `HelpConfig.vue` template, before the closing `</div>`:

```vue
<USeparator class="my-4" />
<HelpTelegram />
```

**Step 1: Commit**

```bash
git add web/app/components/HelpConfig.vue
git commit -m "Include Telegram setup guide in Config help"
```

---

### Task 7: Build, verify, push

**Step 1: Build locally and verify**

```bash
cd web && npm run dev
```

Verify:
- Dashboard shows WelcomeLanding when no device connected
- Help `?` icon appears in header
- Clicking it opens slideover with page-specific content
- Navigating between pages changes the help content
- Provision page help shows hardware requirements and troubleshooting
- Config page help includes Telegram setup guide

**Step 2: Generate static build and verify**

```bash
cd web && NUXT_APP_BASE_URL=/zenclaw/ npx nuxt generate
```

**Step 3: Commit all remaining changes and push**

```bash
git add -A
git commit -m "Web help system: welcome landing + contextual help drawer"
git push
```
