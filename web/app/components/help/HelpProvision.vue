<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Provisioning</h3>
    <p>
      Set up a new ZenClaw device from scratch in three steps.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Step 1 — Configure</h4>
        <p class="text-muted">
          Enter your device name, LLM API key, and (for WiFi boards) your
          network credentials. These are written directly to the device's
          non-volatile storage (NVS) and persist across reboots. Ethernet
          boards like the Guition P4 skip the WiFi fields — just plug in.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Step 2 — Flash</h4>
        <p class="text-muted">
          The browser flashes the firmware image to the ESP32 via Web Serial.
          No drivers or software installation needed — it all happens in
          Chrome or Edge.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Step 3 — Connect</h4>
        <p class="text-muted">
          After flashing, the device boots, joins the network (WiFi for S3,
          Ethernet for P4), and becomes reachable at
          <code>devicename.local</code>. Return to the dashboard and enter
          the hostname to connect.
        </p>
      </div>
    </div>

    <UAlert icon="i-lucide-cpu" title="Hardware">
      <template #description>
        ZenClaw runs on ESP32-S3 (WiFi, native USB, $3–8 — look for USB-OTG
        support) or ESP32-P4 (wired Ethernet, more horsepower — e.g.&nbsp;the
        Guition JC-ESP32P4-M3-DEV). Pick your board in the wizard; it verifies
        the connected chip matches before flashing.
      </template>
    </UAlert>

    <UAlert icon="i-lucide-alert-triangle" color="warning" title="Troubleshooting">
      <template #description>
        <div class="space-y-2">
          <p><strong>Blank device / won't flash:</strong> Hold BOOT, press RESET,
          then release BOOT. The device enters bootloader mode and the serial port
          changes to PID <code>0x0002</code>.</p>
          <p><strong>Linux — permission denied:</strong> Add your user to the
          <code>dialout</code> group:
          <code>sudo usermod -aG dialout $USER</code>, then log out and back in.</p>
          <p><strong>Device not found after flash:</strong> Press RESET. The device
          needs a manual reset after flashing to boot into application mode.</p>
        </div>
      </template>
    </UAlert>

    <UAlert icon="i-lucide-globe" title="Browser requirement">
      <template #description>
        Web Serial is only available in Chromium-based browsers. Use
        <strong>Chrome</strong> or <strong>Edge</strong>. Firefox and Safari
        do not support Web Serial.
      </template>
    </UAlert>
  </div>
</template>
