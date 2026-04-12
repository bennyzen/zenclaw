<script setup lang="ts">
const { state, connectNetwork } = useConnection()
const router = useRouter()

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
  if (!hostname.value || state.connecting) return
  try {
    // If it looks like a raw address (IP, localhost:port), use as-is; otherwise append .local
    const raw = hostname.value
    const addr = raw.includes('.') || raw.includes(':') ? raw : raw + '.local'
    await connectNetwork(addr)
    router.push('/dashboard')
  } catch { /* error shown via state.error */ }
}
</script>

<template>
  <div class="max-w-2xl mx-auto space-y-8">
    <div class="text-center space-y-3">
      <img :src="`${$config.app.baseURL}zenclaw.webp`" alt="ZenClaw on ESP32-S3" class="mx-auto rounded-lg w-full" />
      <h1 class="text-4xl font-bold">ZenClaw</h1>
      <p class="text-xl text-muted">AI agent on a $3 microcontroller</p>
      <p class="text-sm text-dimmed max-w-lg mx-auto">
        ZenClaw runs a full AI agent loop with tool use, memory, and multi-channel
        messaging on an ESP32-S3. Chat via Telegram, web, or serial — the agent
        handles the rest.
      </p>
    </div>

    <div class="grid gap-4 sm:grid-cols-2">
      <UCard>
        <div class="space-y-3">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-plus-circle" class="text-primary" />
            <h3 class="font-semibold">Provision a new device</h3>
          </div>
          <p class="text-sm text-muted">
            Flash firmware and configure a new ESP32-S3 from your browser using
            Web Serial.
          </p>
          <UButton to="/provision" label="Get Started" icon="i-lucide-arrow-right" trailing />
        </div>
      </UCard>

      <UCard>
        <div class="space-y-3">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-plug" class="text-primary" />
            <h3 class="font-semibold">Connect to device</h3>
          </div>
          <p class="text-sm text-muted">
            Enter your device hostname to connect over your local network.
          </p>
          <div class="flex gap-2">
            <UInput
              v-model="hostname"
              placeholder="zenclaw-wild-crow"
              :disabled="state.connecting"
              class="flex-1"
              @keydown.enter="connect"
            >
              <template v-if="!hostname.includes('.') && !hostname.includes(':')" #trailing>
                <span class="text-xs text-dimmed">.local</span>
              </template>
            </UInput>
            <UButton
              :label="state.connecting ? undefined : 'Connect'"
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
      <h3 class="font-semibold">What you need</h3>
      <div class="grid gap-3 grid-cols-2">
        <div class="flex items-start gap-2 p-3 rounded-lg bg-elevated/50">
          <UIcon name="i-lucide-cpu" class="text-primary mt-0.5 shrink-0" />
          <div>
            <p class="text-sm font-medium">ESP32-S3 board</p>
            <p class="text-xs text-dimmed">$3-8 on AliExpress</p>
          </div>
        </div>
        <div class="flex items-start gap-2 p-3 rounded-lg bg-elevated/50">
          <UIcon name="i-lucide-wifi" class="text-primary mt-0.5 shrink-0" />
          <div>
            <p class="text-sm font-medium">WiFi network</p>
            <p class="text-xs text-dimmed">2.4 GHz, same network as this browser</p>
          </div>
        </div>
        <div class="flex items-start gap-2 p-3 rounded-lg bg-elevated/50">
          <UIcon name="i-lucide-key" class="text-primary mt-0.5 shrink-0" />
          <div>
            <p class="text-sm font-medium">LLM API key</p>
            <p class="text-xs text-dimmed">
              <a href="https://aistudio.google.com/apikey" target="_blank" class="text-primary underline">Get one free</a>
              from Google AI Studio
            </p>
          </div>
        </div>
        <div class="flex items-start gap-2 p-3 rounded-lg bg-elevated/50">
          <UIcon name="i-lucide-cloud" class="text-dimmed mt-0.5 shrink-0" />
          <div>
            <p class="text-sm font-medium">Cloud storage <span class="text-xs font-normal text-dimmed">(optional)</span></p>
            <p class="text-xs text-dimmed">
              <a href="https://dash.cloudflare.com/sign-up" target="_blank" class="text-primary underline">Cloudflare R2</a>
              10 GB free, protects your data
            </p>
          </div>
        </div>
      </div>
    </div>

    <UCallout icon="i-lucide-info" title="Browser requirement">
      Provisioning uses Web Serial, which requires Chrome or Edge on desktop.
      The dashboard and config editor work in any modern browser.
    </UCallout>
  </div>
</template>
