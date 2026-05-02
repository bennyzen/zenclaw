<script setup lang="ts">
import type { TabsItem } from '@nuxt/ui'

const { state, getConfig, saveConfig, getWifi, setWifi } = useConnection()

const loading = ref(false)
const saving = ref(false)
const error = ref<string | null>(null)
const successMsg = ref<string | null>(null)
const rawConfig = ref<Record<string, any>>({})

// API base URLs keyed by OpenRouter provider prefix
const BASE_URLS: Record<string, string> = {
  'google': 'https://generativelanguage.googleapis.com/v1beta',
  'openai': 'https://api.openai.com/v1',
  'anthropic': 'https://api.anthropic.com',
  'x-ai': 'https://api.x.ai/v1',
  'z-ai': 'https://api.z.ai/api/coding/paas/v4',
  'mistralai': 'https://api.mistral.ai',
  'deepseek': 'https://api.deepseek.com/v1',
  'cohere': 'https://api.cohere.com/v2',
  'perplexity': 'https://api.perplexity.ai',
  'meta-llama': 'https://api.llama.com/v1',
  'qwen': 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1',
  'nvidia': 'https://integrate.api.nvidia.com/v1',
  'groq': 'https://api.groq.com/openai/v1',
  'cerebras': 'https://api.cerebras.ai/v1',
  'minimax': 'https://api.minimax.io/v1',
  'amazon': 'https://bedrock-runtime.us-east-1.amazonaws.com',
  'ai21': 'https://api.ai21.com/studio/v1',
  'inflection': 'https://api.inflection.ai/v1',
  'moonshotai': 'https://api.moonshot.cn/v1',
  'stepfun': 'https://api.stepfun.com/v1',
  'baidu': 'https://qianfan.baidubce.com/v2',
  'writer': 'https://api.writer.com/v1',
  'upstage': 'https://api.upstage.ai/v1',
  'rekaai': 'https://api.reka.ai/v1',
  'huggingface': 'https://router.huggingface.co/v1',
  'openrouter': 'https://openrouter.ai/api/v1',
  'liquid': 'https://api.liquid.ai/v1',
  'arcee-ai': 'https://conductor.arcee.ai/v2',
  'inception': 'https://api.inceptionlabs.ai/v1',
  'tencent': 'https://api.lkeap.cloud.tencent.com/v1',
  'bytedance-seed': 'https://ark.cn-beijing.volces.com/api/v3',
}

function slugify(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}

// --- Form fields ---
const providerName = ref('google')
const apiKey = ref('')
const model = ref('')
const baseUrl = ref('')
const agentName = ref('')
const heartbeatEnabled = ref(false)
const searchProvider = ref('google')
const braveApiKey = ref('')
const storageProvider = ref('r2')
const storageAccountId = ref('')
const storageEndpoint = ref('')
const storageAccessKey = ref('')
const storageSecretKey = ref('')
const storageBucket = ref('')
const storageRegion = ref('auto')
const telegramEnabled = ref(false)
const botToken = ref('')
const defaultChatId = ref('')

// --- WiFi ---
const wifiStatus = ref<{
  ssid: string | null
  connected: boolean
  ip: string | null
  rssi: number | null
  hostname?: string | null
} | null>(null)
const wifiSsid = ref('')
const wifiPassword = ref('')
const wifiSaving = ref(false)

// --- Dynamic data from OpenRouter ---
interface ORModel { id: string; name: string; provider: string }
const allModels = ref<ORModel[]>([])
const modelsLoading = ref(false)

async function fetchModels() {
  if (allModels.value.length) return
  modelsLoading.value = true
  try {
    const resp = await fetch('https://openrouter.ai/api/v1/models')
    const data = await resp.json()
    allModels.value = (data.data || []).map((m: any) => {
      const id = m.id || ''
      const provider = id.includes('/') ? id.split('/')[0] : 'unknown'
      return { id, name: m.name || id, provider }
    })
  } catch { /* user can type manually */ }
  modelsLoading.value = false
}

// Derive provider list from fetched models (sorted by model count)
const providerItems = computed(() => {
  const counts: Record<string, number> = {}
  for (const m of allModels.value) {
    counts[m.provider] = (counts[m.provider] || 0) + 1
  }
  const sorted = Object.entries(counts)
    .sort((a, b) => b[1] - a[1])
    .map(([value, count]) => ({ label: `${value} (${count})`, value }))
  sorted.push({ label: 'Custom', value: 'custom' })
  return sorted
})

// Filter models for selected provider
const filteredModels = computed(() => {
  if (providerName.value === 'custom' || !allModels.value.length) return []
  return allModels.value
    .filter(m => m.provider === providerName.value)
    .map(m => {
      const short = m.id.includes('/') ? m.id.split('/').slice(1).join('/') : m.id
      return { label: short, value: short }
    })
})

const isCustomProvider = computed(() => providerName.value === 'custom')

interface Preset {
  slug: string
  provider: string
  model: string
  baseUrl: string
  isActive: boolean
}

const presets = computed<Preset[]>(() => {
  const provs = rawConfig.value?.providers || {}
  const defaultSlug = provs.default
  return Object.entries(provs)
    .filter(([k]) => k !== 'default')
    .map(([slug, entry]: [string, any]) => ({
      slug,
      provider: slug.split('__')[0] || slug,
      model: entry?.model || '',
      baseUrl: entry?.base_url || '',
      isActive: slug === defaultSlug,
    }))
    .sort((a, b) => a.slug.localeCompare(b.slug))
})

const switchOpen = ref(false)
const switchTarget = ref<Preset | null>(null)
const switching = ref(false)

function openSwitch(p: Preset) {
  if (p.isActive) return
  error.value = null
  switchTarget.value = p
  switchOpen.value = true
}

async function confirmSwitch() {
  const target = switchTarget.value
  if (!target) return
  switching.value = true
  error.value = null
  successMsg.value = null
  try {
    const config = {
      ...rawConfig.value,
      providers: { ...(rawConfig.value.providers || {}), default: target.slug },
    }
    await saveConfig(config)
    rawConfig.value = config
    const label = target.model ? `${target.provider} / ${target.model}` : target.provider
    successMsg.value = `Switching to ${label}. Device is rebooting…`
    switchOpen.value = false
  } catch (e: any) {
    error.value = `Switch failed: ${e.message}`
  }
  switching.value = false
}

function stripScheme(url: string): string {
  return url.replace(/^https?:\/\//, '')
}

watch(providerName, (name) => {
  if (BASE_URLS[name]) baseUrl.value = BASE_URLS[name]
})

// --- Tabs ---
const tabs: TabsItem[] = [
  { label: 'LLM Provider', icon: 'i-lucide-brain', slot: 'provider' as const },
  { label: 'Web Search', icon: 'i-lucide-search', slot: 'search' as const },
  { label: 'Storage', icon: 'i-lucide-cloud', slot: 'storage' as const },
  { label: 'Agent', icon: 'i-lucide-bot', slot: 'agent' as const },
  { label: 'Telegram', icon: 'i-lucide-send', slot: 'telegram' as const },
  { label: 'WiFi', icon: 'i-lucide-wifi', slot: 'wifi' as const },
]

// --- Load / Save ---
async function load() {
  loading.value = true
  error.value = null
  successMsg.value = null
  try {
    const config = await getConfig()
    rawConfig.value = config

    const defaultSlug = config.providers?.default || 'google'
    const provider = config.providers?.[defaultSlug] || {}
    apiKey.value = provider.api_key || ''
    model.value = provider.model || ''
    baseUrl.value = provider.base_url || ''

    // Slugs look like "openai__gpt-4o-mini" — strip the suffix to recover
    // the provider identity. Legacy keys ("openai") have no suffix and
    // pass through unchanged.
    const providerKey = defaultSlug.split('__')[0] || 'google'
    if (BASE_URLS[providerKey]) {
      providerName.value = providerKey
    } else {
      const match = Object.entries(BASE_URLS).find(([, url]) => baseUrl.value.includes(url))
      providerName.value = match ? match[0] : 'custom'
    }

    agentName.value = config.agent_name || ''
    heartbeatEnabled.value = config.heartbeat?.enabled || false

    // Only override locally-restored values when the device explicitly returns
    // a config block. Otherwise we'd blow away unsaved typing every time
    // the network reconnects.
    if (config.search) {
      searchProvider.value = config.search.provider || 'google'
      braveApiKey.value = config.search.brave_api_key || ''
    }

    if (config.storage) {
      const stor = config.storage
      storageEndpoint.value = stor.endpoint || ''
      storageAccessKey.value = stor.access_key_id || ''
      storageSecretKey.value = stor.secret_access_key || ''
      storageBucket.value = stor.bucket || ''
      storageRegion.value = stor.region || 'auto'
      const r2Match = storageEndpoint.value.match(/https:\/\/([^.]+)\.r2\.cloudflarestorage\.com/)
      if (r2Match) {
        storageAccountId.value = r2Match[1]!
        storageProvider.value = 'r2'
      } else if (storageEndpoint.value) {
        storageProvider.value = 'other'
      }
    }

    const telegram = config.channels?.telegram
    if (telegram) {
      telegramEnabled.value = telegram.enabled || false
      botToken.value = telegram.bot_token || ''
      defaultChatId.value = telegram.default_chat_id || ''
    }
    // WiFi (separate endpoint, won't fail if unavailable)
    try {
      const wifi = await getWifi()
      wifiStatus.value = wifi
      if (wifi.ssid) wifiSsid.value = wifi.ssid
    } catch { /* WiFi endpoint may not exist on desktop */ }
  } catch (e: any) {
    error.value = e.message
  }
  loading.value = false
}

async function saveWifi() {
  if (!wifiSsid.value.trim()) return
  wifiSaving.value = true
  error.value = null
  successMsg.value = null
  try {
    const result = await setWifi(wifiSsid.value, wifiPassword.value)
    if (result.connected) {
      successMsg.value = `Connected to ${wifiSsid.value} (IP: ${result.ip})`
    } else {
      successMsg.value = 'Credentials saved. Reconnecting...'
    }
    try {
      const wifi = await getWifi()
      wifiStatus.value = wifi
    } catch {}
  } catch (e: any) {
    error.value = `WiFi update failed: ${e.message}`
  }
  wifiSaving.value = false
}

async function save() {
  saving.value = true
  error.value = null
  successMsg.value = null

  const config = { ...rawConfig.value }

  const prov = config.providers || {}
  const providerKey = isCustomProvider.value ? 'custom' : providerName.value
  // Slug-derived key lets multiple presets coexist (openai__gpt-4o-mini,
  // openai__gpt-5, etc.). Falls back to the bare provider name when the
  // model is empty so the form still saves something usable.
  const slug = model.value ? `${providerKey}__${slugify(model.value)}` : providerKey
  prov.default = slug
  const providerConfig = prov[slug] || {}
  providerConfig.api_key = apiKey.value
  providerConfig.model = model.value
  providerConfig.base_url = baseUrl.value
  prov[slug] = providerConfig
  config.providers = prov

  config.agent_name = agentName.value
  config.heartbeat = { ...(config.heartbeat || {}), enabled: heartbeatEnabled.value }

  config.search = {
    provider: searchProvider.value,
    ...(searchProvider.value === 'brave' ? { brave_api_key: braveApiKey.value } : {}),
  }

  if (storageAccessKey.value) {
    const endpoint = storageProvider.value === 'r2'
      ? `https://${storageAccountId.value}.r2.cloudflarestorage.com`
      : storageEndpoint.value
    config.storage = {
      endpoint,
      access_key_id: storageAccessKey.value,
      secret_access_key: storageSecretKey.value,
      bucket: storageBucket.value,
      region: storageProvider.value === 'r2' ? 'auto' : storageRegion.value,
    }
  } else {
    delete config.storage
  }

  const channels = config.channels || {}
  const telegram = channels.telegram || {}
  telegram.enabled = telegramEnabled.value
  telegram.bot_token = botToken.value
  telegram.default_chat_id = defaultChatId.value
  channels.telegram = telegram
  config.channels = channels

  try {
    await saveConfig(config)
    successMsg.value = 'Config saved successfully'
    rawConfig.value = config
  } catch (e: any) {
    error.value = `Save failed: ${e.message}`
  }
  saving.value = false
}

// --- localStorage persistence ---
const LS_KEY = 'zenclaw_config'

function persistToLocal() {
  localStorage.setItem(LS_KEY, JSON.stringify({
    providerName: providerName.value,
    apiKey: apiKey.value,
    model: model.value,
    baseUrl: baseUrl.value,
    agentName: agentName.value,
    heartbeatEnabled: heartbeatEnabled.value,
    searchProvider: searchProvider.value,
    braveApiKey: braveApiKey.value,
    storageProvider: storageProvider.value,
    storageAccountId: storageAccountId.value,
    storageEndpoint: storageEndpoint.value,
    storageAccessKey: storageAccessKey.value,
    storageSecretKey: storageSecretKey.value,
    storageBucket: storageBucket.value,
    storageRegion: storageRegion.value,
    telegramEnabled: telegramEnabled.value,
    botToken: botToken.value,
    defaultChatId: defaultChatId.value,
    wifiSsid: wifiSsid.value,
  }))
}

function restoreFromLocal() {
  try {
    const saved = localStorage.getItem(LS_KEY)
    if (!saved) return
    const d = JSON.parse(saved)
    if (d.providerName) providerName.value = d.providerName
    if (d.apiKey) apiKey.value = d.apiKey
    if (d.model) model.value = d.model
    if (d.baseUrl) baseUrl.value = d.baseUrl
    if (d.agentName) agentName.value = d.agentName
    if (d.heartbeatEnabled != null) heartbeatEnabled.value = d.heartbeatEnabled
    if (d.searchProvider) searchProvider.value = d.searchProvider
    if (d.braveApiKey) braveApiKey.value = d.braveApiKey
    if (d.storageProvider) storageProvider.value = d.storageProvider
    if (d.storageAccountId) storageAccountId.value = d.storageAccountId
    if (d.storageEndpoint) storageEndpoint.value = d.storageEndpoint
    if (d.storageAccessKey) storageAccessKey.value = d.storageAccessKey
    if (d.storageSecretKey) storageSecretKey.value = d.storageSecretKey
    if (d.storageBucket) storageBucket.value = d.storageBucket
    if (d.storageRegion) storageRegion.value = d.storageRegion
    if (d.telegramEnabled != null) telegramEnabled.value = d.telegramEnabled
    if (d.botToken) botToken.value = d.botToken
    if (d.defaultChatId) defaultChatId.value = d.defaultChatId
    if (d.wifiSsid) wifiSsid.value = d.wifiSsid
  } catch { /* ignore corrupt data */ }
}

watch([
  providerName, apiKey, model, baseUrl, agentName, heartbeatEnabled,
  searchProvider, braveApiKey,
  storageProvider, storageAccountId, storageEndpoint, storageAccessKey, storageSecretKey, storageBucket, storageRegion,
  telegramEnabled, botToken, defaultChatId,
  wifiSsid,
], persistToLocal)

onMounted(() => {
  restoreFromLocal()
  fetchModels()
  if (state.networkConnected) load()
})

watch(() => state.networkConnected, (connected) => {
  if (connected) load()
})
</script>

<template>
  <div class="max-w-3xl space-y-6">
    <h1 class="text-2xl font-bold">Configuration</h1>

    <template v-if="state.networkConnected">
      <div v-if="loading" class="flex justify-center py-8">
        <UIcon name="i-lucide-loader-2" class="animate-spin text-2xl text-dimmed" />
      </div>

      <template v-else>
        <p v-if="error" class="text-sm text-red-400">{{ error }}</p>
        <p v-if="successMsg" class="text-sm text-green-400">{{ successMsg }}</p>

        <UTabs :items="tabs" variant="link" class="w-full gap-4">
          <!-- LLM Provider -->
          <template #provider>
            <div class="space-y-4 pt-4">
              <UFormField label="Provider" class="w-full">
                <USelectMenu
                  v-model="providerName"
                  :items="providerItems"
                  value-key="value"
                  class="w-full"
                  size="xl"
                  :loading="modelsLoading && !providerItems.length"
                  placeholder="Select provider..."
                />
              </UFormField>
              <UFormField label="Model" class="w-full">
                <USelectMenu
                  v-if="filteredModels.length || modelsLoading"
                  v-model="model"
                  :items="filteredModels"
                  value-key="value"
                  class="w-full"
                  size="xl"
                  :loading="modelsLoading"
                  :placeholder="modelsLoading ? 'Loading models...' : 'Select model...'"
                />
                <UInput
                  v-else
                  v-model="model"
                  placeholder="model-name"
                  size="xl"
                  class="w-full"
                />
              </UFormField>
              <UFormField label="Base URL" class="w-full">
                <UInput
                  v-model="baseUrl"
                  placeholder="https://..."
                  size="xl"
                  class="w-full"
                  :disabled="!isCustomProvider && !!BASE_URLS[providerName]"
                />
                <template v-if="!isCustomProvider && !BASE_URLS[providerName]" #hint>
                  <span class="text-xs text-amber-400">No known API URL for this provider — enter it manually or use OpenRouter</span>
                </template>
              </UFormField>
              <UFormField label="API Key" class="w-full">
                <UInput v-model="apiKey" type="text" placeholder="API key" size="xl" class="w-full" />
              </UFormField>

              <div v-if="presets.length" class="pt-6 border-t border-default">
                <div class="flex items-center justify-between mb-3">
                  <h3 class="text-sm font-medium">Saved Models</h3>
                  <span class="text-xs text-dimmed">Click a card to switch</span>
                </div>
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <button
                    v-for="p in presets"
                    :key="p.slug"
                    type="button"
                    class="text-left rounded-lg border p-3 transition-colors"
                    :class="p.isActive
                      ? 'border-primary bg-primary/5 cursor-default'
                      : 'border-default hover:border-primary/50 hover:bg-elevated cursor-pointer'"
                    @click="openSwitch(p)"
                  >
                    <div class="flex items-start justify-between gap-2">
                      <div class="min-w-0 flex-1">
                        <div class="flex items-center gap-1.5 mb-1">
                          <span v-if="p.isActive" class="size-1.5 rounded-full bg-primary" />
                          <span class="text-sm font-medium truncate">{{ p.provider }}</span>
                        </div>
                        <p class="text-xs text-muted truncate">{{ p.model || '—' }}</p>
                        <p class="text-xs text-dimmed truncate font-mono">{{ stripScheme(p.baseUrl) || '—' }}</p>
                      </div>
                    </div>
                  </button>
                </div>
              </div>
            </div>
          </template>

          <!-- Web Search -->
          <template #search>
            <div class="space-y-4 pt-4">
              <UFormField label="Search Provider" class="w-full">
                <USelect
                  v-model="searchProvider"
                  class="w-full"
                  size="xl"
                  :items="[
                    { label: 'Google (automatic with Gemini)', value: 'google' },
                    { label: 'Brave Search', value: 'brave' },
                    { label: 'None', value: 'none' },
                  ]"
                />
              </UFormField>
              <p v-if="searchProvider === 'google'" class="text-xs text-dimmed">
                Uses Gemini's built-in Google Search grounding. No extra key needed.
              </p>
              <UFormField v-if="searchProvider === 'brave'" label="Brave API Key" class="w-full">
                <UInput v-model="braveApiKey" type="text" placeholder="BSA..." size="xl" class="w-full" />
              </UFormField>
            </div>
          </template>

          <!-- Storage -->
          <template #storage>
            <div class="space-y-4 pt-4">
              <p class="text-xs text-dimmed">
                Cloud storage gives the agent persistent off-device file storage for documents, exports, and shared data.
              </p>

              <UFormField label="Provider" class="w-full">
                <USelect
                  v-model="storageProvider"
                  class="w-full"
                  size="xl"
                  :items="[
                    { label: 'Cloudflare R2 (10 GB free, no egress fees)', value: 'r2' },
                    { label: 'Other S3-compatible (AWS S3, Backblaze B2, MinIO)', value: 'other' },
                  ]"
                />
              </UFormField>

              <!-- Cloudflare R2 -->
              <template v-if="storageProvider === 'r2'">
                <UFormField label="Account ID" class="w-full">
                  <UInput v-model="storageAccountId" placeholder="e.g. 1a2b3c4d5e6f" size="xl" class="w-full" />
                  <template #hint>
                    <span class="text-xs text-dimmed">Cloudflare dashboard URL: dash.cloudflare.com/<strong>&lt;account-id&gt;</strong>/r2</span>
                  </template>
                </UFormField>
                <UFormField label="Bucket Name" class="w-full">
                  <UInput v-model="storageBucket" placeholder="zenclaw" size="xl" class="w-full" />
                  <template #hint>
                    <span class="text-xs text-dimmed">Create a bucket in R2 > Overview > Create bucket</span>
                  </template>
                </UFormField>
                <UFormField label="Access Key ID" class="w-full">
                  <UInput v-model="storageAccessKey" type="text" placeholder="" size="xl" class="w-full" />
                  <template #hint>
                    <span class="text-xs text-dimmed">R2 > Overview > Manage R2 API Tokens > Create API Token with <strong>Admin Read & Write</strong> permission</span>
                  </template>
                </UFormField>
                <UFormField label="Secret Access Key" class="w-full">
                  <UInput v-model="storageSecretKey" type="text" placeholder="" size="xl" class="w-full" />
                  <template #hint>
                    <span class="text-xs text-dimmed">Shown once when you create the token — copy it then</span>
                  </template>
                </UFormField>

                <UCard variant="subtle" class="w-full">
                  <div class="space-y-2">
                    <p class="text-sm font-medium flex items-center gap-1.5">
                      <UIcon name="i-lucide-shield" class="text-amber-500" />
                      CORS Setup Required
                    </p>
                    <p class="text-xs text-muted">
                      The File Manager browses cloud storage directly from your browser. For this to work, your R2 bucket needs CORS rules.
                    </p>
                    <p class="text-xs text-muted">
                      Go to <strong>Cloudflare dashboard > R2 > {{ storageBucket || 'your-bucket' }} > Settings > CORS Policy</strong> and add:
                    </p>
                    <pre class="text-xs bg-elevated rounded p-2 overflow-x-auto">[{
  "AllowedOrigins": ["*"],
  "AllowedMethods": ["GET", "PUT", "DELETE", "HEAD"],
  "AllowedHeaders": ["*"],
  "MaxAgeSeconds": 3600
}]</pre>
                  </div>
                </UCard>
              </template>

              <!-- Other S3-compatible -->
              <template v-else>
                <UFormField label="Endpoint URL" class="w-full">
                  <UInput v-model="storageEndpoint" placeholder="https://s3.us-east-005.backblazeb2.com" size="xl" class="w-full" />
                </UFormField>
                <UFormField label="Bucket Name" class="w-full">
                  <UInput v-model="storageBucket" placeholder="zenclaw" size="xl" class="w-full" />
                </UFormField>
                <UFormField label="Access Key ID" class="w-full">
                  <UInput v-model="storageAccessKey" type="text" placeholder="" size="xl" class="w-full" />
                </UFormField>
                <UFormField label="Secret Access Key" class="w-full">
                  <UInput v-model="storageSecretKey" type="text" placeholder="" size="xl" class="w-full" />
                </UFormField>
                <UFormField label="Region" class="w-full">
                  <UInput v-model="storageRegion" placeholder="us-east-1" size="xl" class="w-full" />
                </UFormField>

                <UCard variant="subtle" class="w-full">
                  <div class="space-y-2">
                    <p class="text-sm font-medium flex items-center gap-1.5">
                      <UIcon name="i-lucide-shield" class="text-amber-500" />
                      CORS Setup Required
                    </p>
                    <p class="text-xs text-muted">
                      The File Manager browses cloud storage directly from your browser via presigned URLs. Your S3 bucket needs CORS configured to allow browser access.
                    </p>
                    <pre class="text-xs bg-elevated rounded p-2 overflow-x-auto">[{
  "AllowedOrigins": ["*"],
  "AllowedMethods": ["GET", "PUT", "DELETE", "HEAD"],
  "AllowedHeaders": ["*"],
  "MaxAgeSeconds": 3600
}]</pre>
                  </div>
                </UCard>
              </template>
            </div>
          </template>

          <!-- Agent -->
          <template #agent>
            <div class="space-y-4 pt-4">
              <UFormField label="Agent Name" class="w-full">
                <UInput v-model="agentName" placeholder="ZenClaw" size="xl" class="w-full" />
              </UFormField>
              <div class="flex items-center gap-3">
                <USwitch v-model="heartbeatEnabled" />
                <span class="text-sm">Heartbeat enabled</span>
              </div>
            </div>
          </template>

          <!-- Telegram -->
          <template #telegram>
            <div class="space-y-4 pt-4">
              <div class="flex items-center gap-3">
                <USwitch v-model="telegramEnabled" />
                <span class="text-sm">Telegram enabled</span>
              </div>
              <UFormField label="Bot Token" class="w-full">
                <UInput v-model="botToken" type="text" placeholder="123456:ABC-DEF..." size="xl" class="w-full" />
              </UFormField>
              <UFormField label="Default Chat ID" class="w-full">
                <UInput v-model="defaultChatId" placeholder="123456789" size="xl" class="w-full" />
              </UFormField>
            </div>
          </template>

          <!-- WiFi -->
          <template #wifi>
            <div class="space-y-4 pt-4">
              <!-- Status -->
              <div v-if="wifiStatus" class="rounded-lg border border-default p-4 space-y-2">
                <div class="flex items-center gap-2">
                  <span class="text-sm text-muted">Status:</span>
                  <UBadge
                    :color="wifiStatus.connected ? 'success' : 'error'"
                    variant="subtle"
                    size="sm"
                  >
                    {{ wifiStatus.connected ? 'Connected' : 'Disconnected' }}
                  </UBadge>
                </div>
                <div v-if="wifiStatus.ssid" class="flex items-center gap-2">
                  <span class="text-sm text-muted">SSID:</span>
                  <span class="text-sm">{{ wifiStatus.ssid }}</span>
                </div>
                <div v-if="wifiStatus.hostname" class="flex items-center gap-2">
                  <span class="text-sm text-muted">Hostname:</span>
                  <span class="text-sm font-mono">{{ wifiStatus.hostname }}.local</span>
                </div>
                <div v-if="wifiStatus.ip" class="flex items-center gap-2">
                  <span class="text-sm text-muted">IP:</span>
                  <span class="text-sm font-mono">{{ wifiStatus.ip }}</span>
                </div>
                <div v-if="wifiStatus.rssi != null" class="flex items-center gap-2">
                  <span class="text-sm text-muted">Signal:</span>
                  <span class="text-sm">{{ wifiStatus.rssi }} dBm</span>
                </div>
              </div>

              <!-- Credentials -->
              <UFormField label="SSID" class="w-full">
                <UInput v-model="wifiSsid" placeholder="Network name" size="xl" class="w-full" />
              </UFormField>
              <UFormField label="Password" class="w-full">
                <UInput v-model="wifiPassword" type="password" placeholder="Network password" size="xl" class="w-full" />
              </UFormField>
              <div class="flex justify-end">
                <UButton
                  :label="wifiSaving ? 'Saving...' : 'Save & Reconnect'"
                  size="xl"
                  :disabled="wifiSaving"
                  @click="saveWifi"
                >
                  <template #leading>
                    <UIcon v-if="wifiSaving" name="i-lucide-loader-circle" class="size-6 animate-spin" />
                    <UIcon v-else name="i-lucide-wifi" class="size-6" />
                  </template>
                </UButton>
              </div>
            </div>
          </template>
        </UTabs>

        <!-- Actions -->
        <div class="flex justify-end gap-3">
          <UButton
            label="Reload"
            icon="i-lucide-refresh-cw"
            size="xl"
            variant="outline"
            color="neutral"
            @click="load"
          />
          <UButton
            :label="saving ? 'Saving...' : 'Save Config'"
            size="xl"
            :disabled="saving"
            @click="save"
          >
            <template #leading>
              <UIcon v-if="saving" name="i-lucide-loader-circle" class="size-6 animate-spin" />
              <UIcon v-else name="i-lucide-save" class="size-6" />
            </template>
          </UButton>
        </div>

        <UModal v-model:open="switchOpen" title="Switch active model?">
          <template #body>
            <p class="text-sm text-default">
              Switch the active model to
              <span class="font-medium">{{ switchTarget?.provider }} / {{ switchTarget?.model }}</span>?
            </p>
            <p class="mt-3 text-sm text-muted">
              The device will reboot to apply the change (~12 seconds offline).
            </p>
          </template>
          <template #footer>
            <div class="flex justify-end gap-2 w-full">
              <UButton
                label="Cancel"
                variant="ghost"
                color="neutral"
                :disabled="switching"
                @click="switchOpen = false"
              />
              <UButton
                label="Switch & Reboot"
                color="primary"
                :loading="switching"
                :disabled="switching"
                @click="confirmSwitch"
              />
            </div>
          </template>
        </UModal>
      </template>
    </template>
  </div>
</template>
