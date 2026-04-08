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
</script>

<template>
  <footer class="border-t border-default bg-elevated px-2 py-0.5">
    <div v-if="state.mode === 'disconnected'" class="flex items-center justify-center text-xs text-dimmed h-6">
      No device connected
    </div>
    <div v-else class="statusbar flex items-center text-xs text-toned h-6">
      <!-- Device IP -->
      <UTooltip>
        <a
          v-if="state.deviceIp"
          :href="`${state.useTls ? 'https' : 'http'}://${state.deviceIp}${state.useTls ? ':' + state.devicePort : ''}`"
          target="_blank"
          class="cell text-muted hover:text-default transition-colors"
        >
          <UIcon name="i-lucide-cpu" :class="modeColor === 'success' ? 'text-green-400' : 'text-blue-400'" class="size-3" />
          {{ state.deviceIp }}
        </a>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-cpu" class="size-4" /> Device Connection</div>
            <div class="tip-row"><span class="tip-label">IP Address</span><span>{{ state.deviceIp }}</span></div>
            <div class="tip-row"><span class="tip-label">Protocol</span><span>{{ state.useTls ? 'HTTPS :' + state.devicePort : 'HTTP :80' }}</span></div>
            <div class="tip-row"><span class="tip-label">Mode</span><span>{{ state.mode === 'both' ? 'USB + Network' : state.mode === 'serial' ? 'USB' : 'Network' }}</span></div>
            <div class="tip-hint">Click to open device landing page</div>
          </div>
        </template>
      </UTooltip>

      <!-- RAM -->
      <UTooltip v-if="ramPercent != null">
        <span class="cell">
          <UIcon name="i-lucide-memory-stick" class="size-3 text-dimmed" />
          {{ ramPercent }}%
        </span>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-memory-stick" class="size-4" /> RAM Usage</div>
            <div class="tip-row"><span class="tip-label">Used</span><span>{{ state.lastStatus?.memory?.usedKb }} KB</span></div>
            <div class="tip-row"><span class="tip-label">Free</span><span>{{ state.lastStatus?.memory?.freeKb }} KB</span></div>
            <div class="tip-row"><span class="tip-label">Total</span><span>{{ state.lastStatus?.memory?.totalKb }} KB</span></div>
            <div class="tip-hint">ESP32-S3 SRAM + PSRAM combined</div>
          </div>
        </template>
      </UTooltip>

      <!-- Device Storage -->
      <UTooltip v-if="state.lastStatus?.storage">
        <span class="cell">
          <UIcon name="i-lucide-hard-drive" class="size-3 text-dimmed" />
          {{ prettyBytes(state.lastStatus.storage.freeKb * 1024) }}
        </span>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-hard-drive" class="size-4" /> Device Storage</div>
            <div class="tip-row"><span class="tip-label">Free</span><span>{{ prettyBytes(state.lastStatus!.storage!.freeKb * 1024) }}</span></div>
            <div class="tip-row"><span class="tip-label">Total</span><span>{{ prettyBytes(state.lastStatus!.storage!.totalKb * 1024) }}</span></div>
            <div class="tip-hint">On-board flash filesystem (sessions, memory, skills)</div>
          </div>
        </template>
      </UTooltip>

      <!-- Cloud Storage -->
      <UTooltip v-if="state.lastStatus?.cloudStorage?.configured">
        <span class="cell">
          <UIcon name="i-lucide-cloud" class="size-3 text-green-400" />
          {{ state.lastStatus.cloudStorage.objects }}&thinsp;obj&ensp;{{ prettyBytes(state.lastStatus.cloudStorage.totalBytes ?? 0) }}
        </span>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-cloud" class="size-4 text-green-400" /> Cloud Storage</div>
            <div class="tip-row"><span class="tip-label">Provider</span><span>{{ state.lastStatus!.cloudStorage!.provider === 'r2' ? 'Cloudflare R2' : 'S3-compatible' }}</span></div>
            <div class="tip-row"><span class="tip-label">Bucket</span><span>{{ state.lastStatus!.cloudStorage!.bucket }}</span></div>
            <div class="tip-row"><span class="tip-label">Objects</span><span>{{ state.lastStatus!.cloudStorage!.objects }}</span></div>
            <div class="tip-row"><span class="tip-label">Size</span><span>{{ prettyBytes(state.lastStatus!.cloudStorage!.totalBytes ?? 0) }}</span></div>
            <div class="tip-hint">Persistent off-device storage for documents and exports</div>
          </div>
        </template>
      </UTooltip>

      <!-- Temperature -->
      <UTooltip v-if="state.lastStatus?.temperatureC != null">
        <span class="cell">
          <UIcon name="i-lucide-thermometer" class="size-3 text-dimmed" />
          {{ state.lastStatus.temperatureC }}&deg;
        </span>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-thermometer" class="size-4" /> MCU Temperature</div>
            <div class="tip-row"><span class="tip-label">Current</span><span>{{ state.lastStatus!.temperatureC }}&deg;C</span></div>
            <div class="tip-hint">ESP32-S3 internal sensor. Normal range: 30-60&deg;C</div>
          </div>
        </template>
      </UTooltip>

      <!-- Uptime -->
      <UTooltip v-if="uptime">
        <span class="cell">
          <UIcon name="i-lucide-clock" class="size-3 text-dimmed" />
          {{ uptime }}
        </span>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-clock" class="size-4" /> Uptime</div>
            <div class="tip-row"><span class="tip-label">Since boot</span><span>{{ uptime }}</span></div>
            <div class="tip-hint">Time since last device restart or power cycle</div>
          </div>
        </template>
      </UTooltip>

      <!-- WiFi Signal -->
      <UTooltip v-if="state.lastStatus?.wifi?.rssi != null">
        <span class="cell">
          <UIcon name="i-lucide-wifi" class="size-3 text-dimmed" />
          {{ state.lastStatus.wifi.rssi }}&thinsp;dBm
        </span>
        <template #content>
          <div class="tip">
            <div class="tip-title"><UIcon name="i-lucide-wifi" class="size-4" /> WiFi Signal</div>
            <div class="tip-row"><span class="tip-label">RSSI</span><span>{{ state.lastStatus!.wifi!.rssi }} dBm</span></div>
            <div class="tip-row">
              <span class="tip-label">Quality</span>
              <span>{{ state.lastStatus!.wifi!.rssi! > -50 ? 'Excellent' : state.lastStatus!.wifi!.rssi! > -60 ? 'Good' : state.lastStatus!.wifi!.rssi! > -70 ? 'Fair' : 'Weak' }}</span>
            </div>
            <div class="tip-hint">-30 dBm = best, -80 dBm = barely usable</div>
          </div>
        </template>
      </UTooltip>

      <!-- Disconnect -->
      <span class="ml-auto">
        <UTooltip text="Disconnect from device">
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

<style>
.tip {
  min-width: 180px;
  font-size: 0.75rem;
  line-height: 1.4;
}
.tip-title {
  display: flex;
  align-items: center;
  gap: 0.375rem;
  font-weight: 600;
  font-size: 0.8125rem;
  margin-bottom: 0.375rem;
  padding-bottom: 0.375rem;
  border-bottom: 1px solid rgb(255 255 255 / 0.1);
}
.tip-row {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
  padding: 0.125rem 0;
}
.tip-label {
  opacity: 0.5;
}
.tip-hint {
  margin-top: 0.375rem;
  padding-top: 0.375rem;
  border-top: 1px solid rgb(255 255 255 / 0.1);
  opacity: 0.4;
  font-size: 0.6875rem;
}
</style>
