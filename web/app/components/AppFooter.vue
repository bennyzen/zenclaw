<script setup lang="ts">
import prettyBytes from 'pretty-bytes'

const { state, disconnectNetwork } = useConnection()

const modeColor = computed(() => {
  switch (state.mode) {
    case 'network': return 'success' as const
    case 'serial': return 'info' as const
    case 'both': return 'success' as const
    default: return 'neutral' as const
  }
})

const ramPercent = computed(() => {
  const mem = state.lastStatus?.memory
  if (!mem || !mem.totalKb) return null
  return Math.round((mem.usedKb / mem.totalKb) * 100)
})

const uptime = computed(() => {
  const s = state.lastStatus?.uptimeS
  if (s == null) return null
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  return `${h}h ${m}m ${sec}s`
})

const tooltipUi = {
  content: 'bg-elevated text-default shadow-lg rounded-md ring ring-default px-3 py-2 text-xs w-56 h-auto',
}
</script>

<template>
  <footer class="border-t border-default bg-elevated px-2 py-0.5">
    <div v-if="state.mode === 'disconnected'" class="flex items-center justify-center text-xs text-dimmed h-6">
      No device connected
    </div>
    <div v-else class="statusbar flex items-center text-xs text-toned h-6">
      <UTooltip :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <a
          v-if="state.deviceIp"
          :href="'http://' + state.deviceIp"
          target="_blank"
          class="cell text-muted hover:text-default transition-colors"
        >
          <UIcon name="i-lucide-cpu" :class="modeColor === 'success' ? 'text-green-400' : 'text-blue-400'" class="size-3" />
          {{ state.deviceIp }}
        </a>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-cpu" class="size-4" /> Device
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">IP</span><span>{{ state.deviceIp }}</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Protocol</span><span>{{ state.useTls ? 'HTTPS' : 'HTTP' }}</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Mode</span><span>{{ state.mode === 'both' ? 'USB + Network' : state.mode === 'serial' ? 'USB' : 'Network' }}</span></div>
          </div>
        </template>
      </UTooltip>

      <UTooltip v-if="ramPercent != null" :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <span class="cell">
          <UIcon name="i-lucide-memory-stick" class="size-3 text-dimmed" />
          {{ ramPercent }}%
        </span>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-memory-stick" class="size-4" /> RAM
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Used</span><span>{{ state.lastStatus?.memory?.usedKb }} KB</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Free</span><span>{{ state.lastStatus?.memory?.freeKb }} KB</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Total</span><span>{{ state.lastStatus?.memory?.totalKb }} KB</span></div>
          </div>
        </template>
      </UTooltip>

      <UTooltip v-if="state.lastStatus?.storage" :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <span class="cell">
          <UIcon name="i-lucide-hard-drive" class="size-3 text-dimmed" />
          {{ prettyBytes(state.lastStatus.storage.freeKb * 1024) }}
        </span>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-hard-drive" class="size-4" /> Storage
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Free</span><span>{{ prettyBytes(state.lastStatus!.storage!.freeKb * 1024) }}</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Total</span><span>{{ prettyBytes(state.lastStatus!.storage!.totalKb * 1024) }}</span></div>
          </div>
        </template>
      </UTooltip>

      <UTooltip v-if="state.lastStatus?.cloudStorage?.configured" :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <span class="cell">
          <UIcon name="i-lucide-cloud" class="size-3 text-green-400" />
          {{ state.lastStatus.cloudStorage.objects }} obj {{ prettyBytes(state.lastStatus.cloudStorage.totalBytes ?? 0) }}
        </span>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-cloud" class="size-4 text-green-400" /> Cloud
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Provider</span><span>{{ state.lastStatus!.cloudStorage!.provider === 'r2' ? 'Cloudflare R2' : 'S3' }}</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Bucket</span><span>{{ state.lastStatus!.cloudStorage!.bucket }}</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Objects</span><span>{{ state.lastStatus!.cloudStorage!.objects }}</span></div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Size</span><span>{{ prettyBytes(state.lastStatus!.cloudStorage!.totalBytes ?? 0) }}</span></div>
          </div>
        </template>
      </UTooltip>

      <span v-if="state.lastStatus?.model" class="cell text-muted">
        <UIcon name="i-lucide-bot" class="size-3 text-dimmed" />
        {{ [state.lastStatus.provider, state.lastStatus.model].filter(Boolean).join('/') }}
      </span>

      <UTooltip v-if="state.lastStatus?.temperatureC != null" :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <span class="cell">
          <UIcon name="i-lucide-thermometer" class="size-3 text-dimmed" />
          {{ state.lastStatus.temperatureC }}&deg;
        </span>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-thermometer" class="size-4" /> Temperature
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">MCU</span><span>{{ state.lastStatus!.temperatureC }}&deg;C</span></div>
          </div>
        </template>
      </UTooltip>

      <UTooltip v-if="uptime" :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <span class="cell">
          <UIcon name="i-lucide-clock" class="size-3 text-dimmed" />
          {{ uptime }}
        </span>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-clock" class="size-4" /> Uptime
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">Since boot</span><span>{{ uptime }}</span></div>
          </div>
        </template>
      </UTooltip>

      <UTooltip v-if="state.lastStatus?.wifi?.rssi != null" :ui="tooltipUi" :content="{ side: 'top', sideOffset: 8 }">
        <span class="cell">
          <UIcon name="i-lucide-wifi" class="size-3 text-dimmed" />
          {{ state.lastStatus.wifi.rssi }} dBm
        </span>
        <template #content>
          <div class="space-y-1.5">
            <div class="flex items-center gap-1.5 font-semibold text-[13px] pb-1.5 border-b border-default">
              <UIcon name="i-lucide-wifi" class="size-4" /> WiFi
            </div>
            <div class="flex justify-between gap-4"><span class="text-dimmed">RSSI</span><span>{{ state.lastStatus!.wifi!.rssi }} dBm</span></div>
            <div class="flex justify-between gap-4">
              <span class="text-dimmed">Quality</span>
              <span>{{ state.lastStatus!.wifi!.rssi! > -50 ? 'Excellent' : state.lastStatus!.wifi!.rssi! > -60 ? 'Good' : state.lastStatus!.wifi!.rssi! > -70 ? 'Fair' : 'Weak' }}</span>
            </div>
          </div>
        </template>
      </UTooltip>

      <span class="ml-auto">
        <UTooltip text="Disconnect" :content="{ side: 'top', sideOffset: 8 }">
          <UButton
            icon="i-lucide-unplug"
            size="xs"
            variant="ghost"
            color="neutral"
            @click="disconnectNetwork"
          />
        </UTooltip>
      </span>
    </div>
  </footer>
</template>

<style scoped>
.statusbar .cell {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  padding: 0 0.5rem;
  border-right: 1px solid rgb(255 255 255 / 0.15);
  align-self: stretch;
}
.statusbar .cell:first-child {
  padding-left: 0;
}
</style>
