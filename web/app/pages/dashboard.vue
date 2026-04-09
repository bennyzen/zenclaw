<script setup lang="ts">
const { state, restartDevice } = useConnection()

const restarting = ref(false)

async function restart() {
  restarting.value = true
  try {
    await restartDevice()
  } catch { /* device will reset, connection will drop */ }
  restarting.value = false
}

const ramPercent = computed(() => {
  const mem = state.lastStatus?.memory
  if (!mem || !mem.totalKb) return 0
  return Math.round((mem.usedKb / mem.totalKb) * 100)
})

const storagePercent = computed(() => {
  const s = state.lastStatus?.storage
  if (!s || !s.totalKb) return 0
  return Math.round(((s.totalKb - s.freeKb) / s.totalKb) * 100)
})

const cloud = computed(() => state.lastStatus?.cloudStorage)

import prettyBytes from 'pretty-bytes'

const deviceFrameKey = ref(0)
const deviceUrl = computed(() => {
  if (!state.deviceIp) return 'about:blank'
  return `http://${state.deviceIp}`
})
let retryTimer: ReturnType<typeof setTimeout> | null = null

function retryFrame() {
  if (retryTimer) return
  retryTimer = setTimeout(() => {
    retryTimer = null
    deviceFrameKey.value++
  }, 5000)
}

onUnmounted(() => {
  if (retryTimer) clearTimeout(retryTimer)
})
</script>

<template>
  <div class="max-w-3xl space-y-6">
    <h1 class="text-2xl font-bold">Dashboard</h1>

    <template v-if="state.networkConnected && state.lastStatus">
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <UCard>
          <div class="space-y-1">
            <p class="text-sm text-muted">Device</p>
            <p class="text-lg font-semibold">{{ state.lastStatus.agentName }}</p>
            <p class="text-sm text-dimmed">v{{ state.lastStatus.version }}</p>
          </div>
        </UCard>

        <UCard>
          <div class="space-y-2">
            <p class="text-sm text-muted">Memory</p>
            <UProgress :model-value="ramPercent" color="primary" size="sm" />
            <p class="text-sm text-dimmed">
              {{ state.lastStatus.memory?.usedKb }} / {{ state.lastStatus.memory?.totalKb }} KB
            </p>
          </div>
        </UCard>

        <UCard>
          <div class="space-y-2">
            <p class="text-sm text-muted">Device Storage</p>
            <UProgress :model-value="storagePercent" color="primary" size="sm" />
            <p class="text-sm text-dimmed">
              {{ state.lastStatus.storage?.freeKb }} KB free
            </p>
          </div>
        </UCard>

        <UCard>
          <div class="space-y-2">
            <p class="text-sm text-muted">Cloud Storage</p>
            <template v-if="cloud?.configured">
              <div class="flex items-center gap-2">
                <UIcon name="i-lucide-cloud" class="text-green-400" />
                <span class="text-sm font-semibold">{{ cloud.bucket }}</span>
              </div>
              <p class="text-sm text-dimmed">
                {{ cloud.objects }} objects, {{ prettyBytes(cloud.totalBytes ?? 0) }}
              </p>
            </template>
            <template v-else>
              <div class="flex items-center gap-2">
                <UIcon name="i-lucide-cloud-off" class="text-dimmed" />
                <span class="text-sm text-dimmed">Not configured</span>
              </div>
              <NuxtLink to="/config" class="text-xs text-primary hover:underline">
                Add cloud storage for 10 GB free
              </NuxtLink>
            </template>
          </div>
        </UCard>
      </div>

      <UCard>
        <template #header>
          <span class="font-semibold">Quick Actions</span>
        </template>
        <div class="flex flex-wrap gap-3">
          <UButton
            to="/files"
            label="File Manager"
            icon="i-lucide-folder"
            size="xl"
            variant="outline"
            color="neutral"
          />
          <UButton
            to="/config"
            label="Config"
            icon="i-lucide-settings"
            size="xl"
            variant="outline"
            color="neutral"
          />
          <UButton
            :label="restarting ? 'Restarting...' : 'Restart Device'"
            size="xl"
            variant="outline"
            color="error"
            :disabled="restarting"
            @click="restart"
          >
            <template #leading>
              <UIcon v-if="restarting" name="i-lucide-loader-circle" class="size-6 animate-spin" />
              <UIcon v-else name="i-lucide-rotate-ccw" class="size-6" />
            </template>
          </UButton>
        </div>
      </UCard>

      <UCard>
        <template #header>
          <div class="flex items-center justify-between">
            <span class="font-semibold">Device</span>
            <UTooltip text="Reload">
              <UButton
                icon="i-lucide-refresh-cw"
                size="xs"
                variant="ghost"
                color="neutral"
                @click="deviceFrameKey++"
              />
            </UTooltip>
          </div>
        </template>
        <iframe
          :key="deviceFrameKey"
          :src="deviceUrl"
          class="w-full rounded border border-default bg-black"
          style="height: 200px"
          @error="retryFrame"
        />
      </UCard>
    </template>

    <div v-else class="text-center py-12 text-dimmed">
      <UIcon name="i-lucide-unplug" class="size-8 mb-2" />
      <p>No device connected</p>
      <p class="text-sm mt-1">Enter a hostname in the bar above or <NuxtLink to="/provision" class="text-primary underline">provision a new device</NuxtLink></p>
    </div>
  </div>
</template>
