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
          Enter your WiFi credentials and LLM API key. These are written
          directly to the device's non-volatile storage (NVS) and persist
          across reboots.
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
          After flashing, the device boots, connects to WiFi, and becomes
          reachable at <code>devicename.local</code>. Return to the dashboard
          and enter the hostname to connect.
        </p>
      </div>
    </div>

    <UCallout icon="i-lucide-cpu" title="Hardware">
      ZenClaw targets the ESP32-S3 with native USB support. These boards cost
      $3–8 and are widely available. Look for "ESP32-S3" with USB-OTG support.
    </UCallout>

    <UCallout icon="i-lucide-alert-triangle" color="warning" title="Troubleshooting">
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
    </UCallout>

    <UCallout icon="i-lucide-globe" title="Browser requirement">
      Web Serial is only available in Chromium-based browsers. Use
      <strong>Chrome</strong> or <strong>Edge</strong>. Firefox and Safari
      do not support Web Serial.
    </UCallout>
  </div>
</template>
