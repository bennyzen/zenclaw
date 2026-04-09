<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Dashboard</h3>
    <p>
      The dashboard gives you a quick overview of your ZenClaw device status and
      access to common actions.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Device Card</h4>
        <p class="text-muted">Shows the agent name and firmware version running on the device.</p>
      </div>

      <div>
        <h4 class="font-medium">Memory</h4>
        <p class="text-muted">
          Displays current RAM usage as a percentage and bar. Normal operating
          range is 40–70%. Values above 85% may indicate memory pressure — the
          agent runs garbage collection automatically.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Device Storage</h4>
        <p class="text-muted">
          Shows the ESP32 flash filesystem contents — firmware files, sessions,
          memory index, and skills.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Cloud Storage</h4>
        <p class="text-muted">
          Off-device backup via S3-compatible storage. Keeps your data safe even
          if the flash is corrupted. Configure in
          <NuxtLink to="/config" class="text-primary underline">Config</NuxtLink>.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Quick Actions</h4>
        <p class="text-muted">
          Shortcuts to File Manager, Config editor, and device restart.
        </p>
      </div>
    </div>

    <USeparator />

    <div>
      <h4 class="font-medium mb-2">Status Bar Metrics</h4>
      <div class="space-y-1 text-muted">
        <p><strong>RAM</strong> — free heap memory on the ESP32</p>
        <p><strong>Storage</strong> — flash filesystem usage</p>
        <p><strong>Cloud</strong> — connection status to S3 storage</p>
        <p><strong>Temperature</strong> — MCU internal temperature sensor</p>
        <p><strong>Uptime</strong> — time since last boot</p>
        <p><strong>WiFi</strong> — connection status and RSSI signal strength</p>
      </div>
    </div>
  </div>
</template>
