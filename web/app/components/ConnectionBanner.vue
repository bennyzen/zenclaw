<script setup lang="ts">
const route = useRoute()
const { state, connectNetwork, disconnectNetwork } = useConnection()

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
  if (hostname.value && !state.networkConnected && !state.connecting && route.path !== '/provision') {
    connect()
  }
})

async function connect() {
  if (!hostname.value) return
  try {
    await connectNetwork(hostname.value + '.local')
    // Persist hostname for next session
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || '{}')
    saved.deviceName = hostname.value
    localStorage.setItem(STORAGE_KEY, JSON.stringify(saved))
  } catch { /* error shown via state.error */ }
}

// Sync hostname field when reconnect succeeds (e.g. auto-reconnect restored connection)
watch(() => state.networkConnected, (connected) => {
  if (connected && state.deviceIp) {
    const name = state.deviceIp.replace('.local', '')
    if (name && name !== hostname.value) hostname.value = name
  }
})
</script>

<template>
  <div v-if="!state.networkConnected && route.path !== '/provision'" class="border-b border-default bg-elevated/50 px-4 py-2">
    <div class="flex items-center gap-3">
      <UIcon name="i-lucide-unplug" class="text-dimmed shrink-0" />
      <UInput
        v-model="hostname"
        placeholder="zenclaw-wild-crow"
        size="sm"
        class="max-w-xs"
        :disabled="state.connecting"
        @keydown.enter="connect"
      >
        <template #trailing>
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
