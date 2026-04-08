<script setup lang="ts">
import type { StepperItem } from '@nuxt/ui'
import type { FlashProgress } from '~/composables/useSerial'

const serial = useSerial()

const adjectives = ['swift', 'bold', 'keen', 'warm', 'calm', 'wild', 'bright', 'quick', 'sharp', 'cool']
const nouns = ['fox', 'owl', 'wolf', 'bear', 'hawk', 'lynx', 'crow', 'deer', 'hare', 'wren']
function randomName(): string {
  const adj = adjectives[Math.floor(Math.random() * adjectives.length)]
  const noun = nouns[Math.floor(Math.random() * nouns.length)]
  return `zenclaw-${adj}-${noun}`
}

const STORAGE_KEY = 'zenclaw_provision'

const active = ref(0)
const progress = ref<FlashProgress>({ stage: 'connecting', percent: 0, message: '' })
const wifiSsid = ref('')
const wifiPassword = ref('')
const apiKey = ref('')
const apiProvider = ref('google')
const apiModel = ref('gemini-2.5-flash')
const deviceName = ref(randomName())
const deviceIp = computed(() => `${deviceName.value}.local`)
const flashing = ref(false)
const polling = ref(false)
const pollStatus = ref('')
const error = ref<string | null>(null)

const logEl = ref<HTMLElement | null>(null)

const serialSupported = computed(() =>
  import.meta.client ? 'serial' in navigator : false,
)

const configValid = computed(() => wifiSsid.value && apiKey.value)

const items: StepperItem[] = [
  { title: 'Configure', description: 'WiFi and API keys', icon: 'i-lucide-settings' },
  { title: 'Flash', description: 'Flash firmware via USB', icon: 'i-lucide-zap' },
  { title: 'Connect', description: 'Waiting for device', icon: 'i-lucide-wifi' },
  { title: 'Done', description: 'Ready to use', icon: 'i-lucide-check' },
]

// Restore from localStorage
onMounted(() => {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (!saved) return
    const data = JSON.parse(saved)
    if (data.wifiSsid) wifiSsid.value = data.wifiSsid
    if (data.wifiPassword) wifiPassword.value = data.wifiPassword
    if (data.apiKey) apiKey.value = data.apiKey
    if (data.apiProvider) apiProvider.value = data.apiProvider
    if (data.apiModel) apiModel.value = data.apiModel
    if (data.deviceName) deviceName.value = data.deviceName
  } catch { /* ignore */ }
})

// Save to localStorage on change
watch([wifiSsid, wifiPassword, apiKey, apiProvider, apiModel, deviceName], () => {
  localStorage.setItem(STORAGE_KEY, JSON.stringify({
    wifiSsid: wifiSsid.value,
    wifiPassword: wifiPassword.value,
    apiKey: apiKey.value,
    apiProvider: apiProvider.value,
    apiModel: apiModel.value,
    deviceName: deviceName.value,
  }))
})

// Auto-scroll log to bottom
watch(() => serial.logs.value.length, () => {
  nextTick(() => {
    if (logEl.value) {
      logEl.value.scrollTop = logEl.value.scrollHeight
    }
  })
})

function nextStep() {
  if (!configValid.value) return
  active.value = 1
}

async function flash() {
  flashing.value = true
  error.value = null

  const ok = await serial.flashDevice(
    { ssid: wifiSsid.value, password: wifiPassword.value, hostname: deviceName.value },
    (p) => { progress.value = p },
  )

  if (ok) {
    active.value = 2
    pollForDevice()
  } else {
    error.value = progress.value.message
  }
  flashing.value = false
}

async function pollForDevice() {
  polling.value = true
  error.value = null
  pollStatus.value = 'Waiting for device to boot and connect to WiFi...'

  const conn = useConnection()
  const maxAttempts = 30
  for (let i = 1; i <= maxAttempts; i++) {
    pollStatus.value = `Trying to reach ${deviceIp.value}... (attempt ${i}/${maxAttempts})`
    try {
      await conn.connectNetwork(deviceName.value + '.local')
      pollStatus.value = `Device online at ${deviceIp.value}!`
      await serial.stopMonitor()
      // Merge provider settings into existing config (preserves telegram, heartbeat, etc.)
      pollStatus.value = 'Pushing API configuration...'
      const existing = await conn.getConfig()
      const providers = existing.providers || {}
      providers.default = apiProvider.value
      providers[apiProvider.value] = {
        ...(providers[apiProvider.value] || {}),
        api_key: apiKey.value,
        model: apiModel.value,
        base_url: apiProvider.value === 'google'
          ? 'https://generativelanguage.googleapis.com/v1beta'
          : 'https://api.openai.com/v1',
      }
      await conn.saveConfig({ ...existing, providers })
      active.value = 3
      polling.value = false
      return
    } catch {
      // Not ready yet
    }
    await new Promise(r => setTimeout(r, 3000))
  }

  polling.value = false
  error.value = `Could not reach ${deviceIp.value} after ${maxAttempts} attempts. Check WiFi credentials and router.`
}
</script>

<template>
  <div class="space-y-6 max-w-3xl">
    <h2 class="text-2xl font-bold text-white">Provision Device</h2>
    <p class="text-sm text-muted">
      Set up a new ZenClaw device in three steps: enter your WiFi and API credentials,
      flash the firmware over USB, and wait for the device to come online.
      Everything is flashed in one go — no serial configuration needed after.
    </p>

    <p v-if="!serialSupported" class="text-red-400">
      Web Serial API is not supported in this browser. Use Chrome or Edge.
    </p>

    <UStepper v-model="active" :items="items" class="w-full">
      <template #content="{ item }">
        <!-- Configure -->
        <div v-if="item.title === 'Configure'" class="space-y-4 pt-10">
          <UFormField label="WiFi SSID" class="w-full">
            <UInput v-model="wifiSsid" placeholder="Your WiFi network" class="w-full" size="xl" />
          </UFormField>
          <UFormField label="WiFi Password" class="w-full">
            <UInput v-model="wifiPassword" class="w-full" size="xl" />
          </UFormField>

          <USeparator />

          <UFormField label="LLM Provider" class="w-full">
            <USelect
              v-model="apiProvider"
              class="w-full"
              size="xl"
              :items="[
                { label: 'Google (Gemini)', value: 'google' },
                { label: 'OpenAI', value: 'openai' },
              ]"
            />
          </UFormField>
          <UFormField label="API Key" class="w-full">
            <UInput v-model="apiKey" placeholder="Your API key" class="w-full" size="xl" />
          </UFormField>
          <UFormField label="Model" class="w-full">
            <UInput v-model="apiModel" class="w-full" size="xl" />
          </UFormField>

          <USeparator />

          <UFormField label="Device Name" class="w-full">
            <UInput v-model="deviceName" class="w-full" size="xl" />
          </UFormField>
          <p class="text-xs text-dimmed">
            Reachable at <strong class="text-muted">{{ deviceName }}.local</strong> on your network
          </p>

          <div class="flex justify-end">
            <UButton :disabled="!configValid" size="xl" icon="i-lucide-arrow-right" @click="nextStep">
              Next
            </UButton>
          </div>
        </div>

        <!-- Flash -->
        <div v-else-if="item.title === 'Flash'" class="space-y-4 pt-10">
          <p class="text-sm text-muted">
            Plug your ESP32-S3 into this computer via USB and click Flash.
            If the device is already running MicroPython, it will reboot into bootloader mode automatically.
          </p>

          <div class="rounded border border-default bg-elevated p-3 text-xs text-muted">
            <p class="font-semibold text-toned mb-1">First-time / blank device:</p>
            <p>If the device has no firmware yet, enter bootloader mode manually:</p>
            <ol class="list-decimal list-inside space-y-0.5 mt-1">
              <li>Hold the <strong>BOOT</strong> button</li>
              <li>Press and release <strong>RESET</strong></li>
              <li>Release <strong>BOOT</strong></li>
            </ol>
            <p class="mt-2">In the port picker, select <strong>USB JTAG/serial debug unit</strong>.</p>
          </div>

          <div class="rounded border border-default bg-elevated p-3 text-xs text-muted">
            <p class="font-semibold text-toned mb-1">Linux users:</p>
            <p>If flashing fails with a permissions error, add yourself to the serial port group (requires logout):</p>
            <code class="mt-1 block text-green-400">sudo usermod -aG uucp $USER &nbsp;# Arch Linux</code>
            <code class="mt-1 block text-green-400">sudo usermod -aG dialout $USER &nbsp;# Debian/Ubuntu</code>
          </div>

          <div v-if="progress.percent > 0" class="space-y-2">
            <p class="text-sm text-muted">{{ progress.message }}</p>
            <UProgress :model-value="progress.percent" :max="100" status />
          </div>

          <div class="flex justify-end">
            <UButton :disabled="!serialSupported || flashing" size="xl" @click="flash">
              <template #leading>
                <UIcon v-if="flashing" name="i-lucide-loader-circle" class="size-6 animate-spin" />
                <UIcon v-else name="i-lucide-zap" class="size-6" />
              </template>
              {{ flashing ? 'Flashing...' : 'Flash Device' }}
            </UButton>
          </div>
        </div>

        <!-- Connect -->
        <div v-else-if="item.title === 'Connect'" class="space-y-4 pt-10">
          <p class="text-sm text-muted">
            Waiting for <a :href="'http://' + deviceIp" target="_blank" class="text-blue-400 underline">{{ deviceIp }}</a> to come online.
            If the device doesn't appear within 30 seconds, press <strong>RST</strong> on the board.
          </p>
          <p v-if="pollStatus" class="text-sm text-dimmed">{{ pollStatus }}</p>
          <UProgress v-if="polling" animation="carousel" />
          <div class="flex justify-end gap-2">
            <UButton
              v-if="!serial.monitoring.value"
              variant="outline"
              color="neutral"
              size="xl"
              icon="i-lucide-terminal"
              @click="serial.startMonitor()"
            >
              Serial Monitor
            </UButton>
            <UButton v-if="!polling" size="xl" icon="i-lucide-refresh-cw" @click="pollForDevice">
              Retry
            </UButton>
          </div>
        </div>

        <!-- Done -->
        <div v-else class="space-y-4 pt-10">
          <p class="text-sm text-muted">
            Your ZenClaw device is running at <a :href="'http://' + deviceIp" target="_blank" class="text-blue-400 underline">{{ deviceIp }}</a> and configured.
          </p>
          <div class="flex justify-end">
            <UButton to="/" size="xl" icon="i-lucide-layout-dashboard">
              Go to Dashboard
            </UButton>
          </div>
        </div>
      </template>
    </UStepper>

    <p v-if="error" class="text-sm text-red-400">{{ error }}</p>

    <!-- Serial log — visible from Flash step onward -->
    <div v-if="active >= 1">
      <div class="flex items-center justify-between mb-2">
        <div class="flex items-center gap-2">
          <h3 class="text-sm font-semibold text-muted">Serial Monitor</h3>
          <UBadge v-if="serial.monitoring.value" color="success" variant="subtle" size="xs">Live</UBadge>
        </div>
        <div class="flex gap-1">
          <UButton
            v-if="serial.monitoring.value"
            size="xs" variant="ghost" color="neutral"
            @click="serial.stopMonitor()"
          >
            Stop
          </UButton>
          <UButton size="xs" variant="ghost" color="neutral" @click="serial.clearLogs()">
            Clear
          </UButton>
        </div>
      </div>
      <div
        ref="logEl"
        class="h-64 overflow-y-auto rounded border border-default bg-black p-3 font-mono text-xs text-green-400"
      >
        <div v-for="(line, i) in serial.logs.value" :key="i">{{ line }}</div>
      </div>
    </div>
  </div>
</template>
