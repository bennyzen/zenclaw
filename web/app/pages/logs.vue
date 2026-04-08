<script setup lang="ts">
const { state, wsUrl } = useConnection()

interface LogEntry {
  level: string
  msg: string
  source: string
  ts: number
}

const entries = ref<LogEntry[]>([])
const maxEntries = 500
const paused = ref(false)
const levelFilter = ref('')
const sourceFilter = ref('')
const autoScroll = ref(true)
let ws: WebSocket | null = null
const logContainer = ref<HTMLElement | null>(null)

const levels = ['info', 'error', 'warning', 'critical', 'debug']

const levelColors: Record<string, string> = {
  info: 'text-blue-400',
  error: 'text-red-400',
  warning: 'text-yellow-400',
  critical: 'text-red-500 font-bold',
  debug: 'text-zinc-500',
}

const sourceColors: Record<string, string> = {}
const palette = [
  'text-cyan-400', 'text-green-400', 'text-purple-400', 'text-orange-400',
  'text-pink-400', 'text-teal-400', 'text-indigo-400', 'text-amber-400',
]
let colorIdx = 0

function sourceColor(source: string): string {
  if (!sourceColors[source]) {
    sourceColors[source] = palette[colorIdx % palette.length]!
    colorIdx++
  }
  return sourceColors[source]!
}

const filtered = computed(() => {
  let list = entries.value
  if (levelFilter.value) {
    list = list.filter(e => e.level === levelFilter.value)
  }
  if (sourceFilter.value) {
    const s = sourceFilter.value.toLowerCase()
    list = list.filter(e => e.source.toLowerCase().includes(s))
  }
  return list
})

function connect() {
  if (ws) return
  const url = `${wsUrl()}/ws/logs`
  ws = new WebSocket(url)
  ws.onmessage = (event) => {
    if (paused.value) return
    try {
      const data = JSON.parse(event.data)
      entries.value.push({
        level: data.level || 'info',
        msg: data.msg || '',
        source: data.source || '',
        ts: Date.now(),
      })
      if (entries.value.length > maxEntries) {
        entries.value = entries.value.slice(-maxEntries)
      }
      if (autoScroll.value) {
        nextTick(() => {
          const el = logContainer.value
          if (el) el.scrollTop = el.scrollHeight
        })
      }
    } catch { /* ignore */ }
  }
  ws.onclose = () => { ws = null }
  ws.onerror = () => { ws?.close(); ws = null }
}

function disconnect() {
  if (ws) { ws.close(); ws = null }
}

function clear() {
  entries.value = []
}

function formatTime(ts: number): string {
  const d = new Date(ts)
  return d.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
    + '.' + String(d.getMilliseconds()).padStart(3, '0')
}

onMounted(() => {
  if (state.networkConnected) connect()
})

watch(() => state.networkConnected, (connected) => {
  if (connected) connect()
  else disconnect()
})

onUnmounted(() => {
  disconnect()
})
</script>

<template>
  <div class="flex flex-col h-[calc(100vh-8rem)]">
    <div class="flex items-center justify-between mb-4">
      <h1 class="text-2xl font-bold">Logs</h1>
      <div class="flex items-center gap-2">
        <UInput
          v-model="sourceFilter"
          placeholder="Filter source..."
          size="sm"
          class="w-40"
          :ui="{ base: 'font-mono' }"
        />
        <USelect
          v-model="levelFilter"
          :items="levels.map(l => ({ label: l.toUpperCase(), value: l }))"
          value-key="value"
          placeholder="All levels"
          size="sm"
          class="w-32"
        />
        <UButton
          :icon="paused ? 'i-lucide-play' : 'i-lucide-pause'"
          :label="paused ? 'Resume' : 'Pause'"
          size="sm"
          :color="paused ? 'primary' : 'neutral'"
          variant="outline"
          @click="paused = !paused"
        />
        <UButton
          icon="i-lucide-trash-2"
          label="Clear"
          size="sm"
          color="neutral"
          variant="outline"
          @click="clear"
        />
        <UButton
          :icon="autoScroll ? 'i-lucide-arrow-down-to-line' : 'i-lucide-arrow-down'"
          :label="autoScroll ? 'Auto-scroll' : 'Manual'"
          size="sm"
          :color="autoScroll ? 'primary' : 'neutral'"
          variant="outline"
          @click="autoScroll = !autoScroll"
        />
      </div>
    </div>

    <div
      v-if="state.networkConnected"
      ref="logContainer"
      class="flex-1 min-h-0 overflow-y-auto rounded-lg border border-default bg-black font-mono text-xs"
    >
      <div v-if="filtered.length === 0" class="p-8 text-center text-dimmed">
        <template v-if="entries.length === 0">
          Waiting for log entries...
        </template>
        <template v-else>
          No entries match filters
        </template>
      </div>

      <div
        v-for="(entry, i) in filtered"
        :key="i"
        class="flex gap-2 px-3 py-0.5 hover:bg-zinc-900/50 border-b border-zinc-900"
      >
        <span class="text-dimmed shrink-0">{{ formatTime(entry.ts) }}</span>
        <span :class="[levelColors[entry.level] || 'text-muted', 'uppercase shrink-0 w-12']">{{ entry.level.slice(0, 4) }}</span>
        <span v-if="entry.source" :class="[sourceColor(entry.source), 'shrink-0']">[{{ entry.source }}]</span>
        <span class="text-toned break-all">{{ entry.msg }}</span>
      </div>
    </div>

    <div class="flex items-center justify-between mt-2 text-xs text-dimmed">
      <span>{{ filtered.length }} entries{{ entries.length !== filtered.length ? ` (${entries.length} total)` : '' }}</span>
      <span v-if="ws" class="flex items-center gap-1">
        <span class="size-2 rounded-full bg-green-500" />
        Connected
      </span>
      <span v-else class="flex items-center gap-1">
        <span class="size-2 rounded-full bg-red-500" />
        Disconnected
      </span>
    </div>
  </div>
</template>
