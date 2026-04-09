<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">File Manager</h3>
    <p>
      Browse and manage files on the device flash and in cloud storage.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Device Files</h4>
        <p class="text-muted">
          Browse the ESP32 flash filesystem. Key directories:
        </p>
        <ul class="list-disc list-inside text-muted mt-1 space-y-1">
          <li><code>data/sessions/</code> — conversation JSONL files</li>
          <li><code>data/memory/</code> — vector memory index and entries</li>
          <li><code>data/cron/</code> — scheduled job definitions</li>
          <li><code>data/skills/</code> — installed skill scripts</li>
        </ul>
      </div>

      <div>
        <h4 class="font-medium">Cloud Files</h4>
        <p class="text-muted">
          Browse your S3-compatible bucket. Files under the <code>sys/</code>
          prefix contain agent data synced from the device. Other files are
          accessible via presigned URLs for direct browser-to-bucket transfers.
        </p>
      </div>
    </div>

    <UCallout icon="i-lucide-cloud" title="Cloud Storage">
      Cloud browsing requires cloud storage to be configured. See
      <NuxtLink to="/config" class="text-primary underline">Config</NuxtLink>
      for setup. Cloudflare R2 offers 10 GB free storage.
    </UCallout>
  </div>
</template>
