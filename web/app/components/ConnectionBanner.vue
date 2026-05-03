<script setup lang="ts">
const route = useRoute()
const { state, connectNetwork, disconnectNetwork } = useConnection()

const STORAGE_KEY = 'zenclaw_provision'
const hostname = ref('')

const isQualified = computed(() => {
  const v = hostname.value.trim()
  return v.includes('.') || v.includes(':')
})

onMounted(() => {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved) {
      const data = JSON.parse(saved)
      if (data.deviceName) hostname.value = data.deviceName
    }
  } catch { /* ignore */ }
  if (hostname.value && !state.networkConnected && !state.connecting && route.path !== '/provision') {
    connect()
  }
})

async function connect() {
  if (!hostname.value) return
  try {
    const { host, port } = parseConnectInput(hostname.value)
    await connectNetwork(host, port)
    // Persist the literal input string so reconnect shows what the user typed.
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || '{}')
    saved.deviceName = hostname.value
    localStorage.setItem(STORAGE_KEY, JSON.stringify(saved))
  } catch { /* error shown via state.error */ }
}

// Only fill the field if it's empty when a connection appears (e.g. the
// banner mounts after another component triggered the connect). Don't
// clobber a user-typed value — that would lose the `:port` they entered.
watch(() => state.networkConnected, (connected) => {
  if (connected && state.deviceIp && !hostname.value) {
    const name = state.deviceIp.endsWith('.local')
      ? state.deviceIp.slice(0, -'.local'.length)
      : state.deviceIp
    hostname.value = state.devicePort === 80 ? name : `${state.deviceIp}:${state.devicePort}`
  }
})
</script>

<template>
  <div v-if="!state.networkConnected && route.path !== '/provision'" class="border-b border-default bg-elevated/50 px-4 py-2">
    <div class="flex items-center gap-3">
      <UIcon name="i-lucide-unplug" class="text-dimmed shrink-0" />
      <UInput
        v-model="hostname"
        placeholder="zenclaw-wild-crow or localhost:8080"
        size="sm"
        class="max-w-xs"
        :disabled="state.connecting"
        @keydown.enter="connect"
      >
        <template v-if="!isQualified" #trailing>
          <span class="text-xs text-dimmed">.local</span>
        </template>
      </UInput>
      <UButton
        :label="state.connecting ? 'Connecting...' : 'Connect'"
        size="sm"
        :disabled="state.connecting || !hostname"
        @click="connect"
      >
        <template #leading>
          <UIcon v-if="state.connecting" name="i-lucide-loader-circle" class="animate-spin" />
          <UIcon v-else name="i-lucide-plug" />
        </template>
      </UButton>
      <span v-if="state.error" class="text-xs text-red-400">{{ state.error }}</span>
    </div>
  </div>
</template>
