<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">File Manager</h3>
    <p>
      Browse and manage files on the device flash and in cloud storage.
      Switch between the two using the <strong>Device</strong> and
      <strong>Cloud</strong> buttons at the top.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Two Storage Tiers</h4>
        <p class="text-muted">
          The ESP32 has limited flash storage. To keep it healthy, files
          are split across two tiers:
        </p>
        <ul class="list-disc list-inside text-muted mt-1 space-y-1">
          <li><strong>Device storage</strong> — small files up to 256 KB
            (configs, notes, scripts). Fast access, available offline.</li>
          <li><strong>Cloud storage (R2)</strong> — any size. Uploaded
            directly from your browser to the bucket via presigned URLs,
            never touching the device. Free tier includes 10 GB.</li>
        </ul>
        <p class="text-muted mt-1">
          When you upload a file larger than 256 KB while in Device mode,
          it is automatically redirected to cloud storage.
        </p>
      </div>

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
        <p class="text-muted mt-1">
          The agent can read and search cloud files of any size using chunked
          access — it never needs to load the full file into device memory.
          For Gemini, files can be streamed directly to Google for full-context
          analysis (storage_analyze tool).
        </p>
      </div>

      <div>
        <h4 class="font-medium">Working with Large Files</h4>
        <p class="text-muted">
          Upload data files, logs, or documents to cloud storage, then ask
          the agent to work with them. Examples:
        </p>
        <ul class="list-disc list-inside text-muted mt-1 space-y-1">
          <li><em>"Search for ERROR in logs/app.log in cloud storage"</em></li>
          <li><em>"Read the first section of data/report.csv"</em></li>
          <li><em>"Analyze the file data/sales.csv from cloud storage"</em> (Gemini)</li>
        </ul>
      </div>
    </div>

    <UCallout icon="i-lucide-cloud" title="Cloud Storage">
      Cloud browsing requires cloud storage to be configured. See
      <NuxtLink to="/config" class="text-primary underline">Config</NuxtLink>
      for setup. Cloudflare R2 offers 10 GB free storage.
    </UCallout>
  </div>
</template>
