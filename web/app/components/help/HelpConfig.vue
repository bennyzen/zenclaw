<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Configuration</h3>
    <p>
      Configure the LLM provider, cloud storage, and Telegram integration.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">LLM Provider</h4>
        <p class="text-muted">
          Choose between Google Gemini or any OpenAI-compatible provider.
          Enter your API key and select a model. For access to many models
          through a single API, try OpenRouter by setting the base URL to
          <code>https://openrouter.ai/api/v1</code>.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Cloud Storage</h4>
        <p class="text-muted">
          The ESP32 flash has limited write cycles. Cloud storage backs up
          sessions, memory, and agent state to an S3-compatible bucket,
          protecting against flash wear and data loss.
        </p>
        <p class="text-muted mt-1">
          Recommended free-tier options:
        </p>
        <ul class="list-disc list-inside text-muted mt-1 space-y-1">
          <li><strong>Cloudflare R2</strong> — 10 GB free, no egress fees</li>
          <li><strong>Backblaze B2</strong> — 10 GB free, low egress cost</li>
        </ul>
      </div>
    </div>

    <USeparator />

    <div>
      <h4 class="font-medium mb-2">Telegram Bot Setup</h4>
      <p class="text-muted">
        See the dedicated <strong>Telegram</strong> section below for a
        step-by-step guide to setting up a Telegram bot for your device.
      </p>
    </div>

    <USeparator />

    <HelpTelegram />

    <UCallout icon="i-lucide-key" title="API keys">
      API keys are stored in <code>config.json</code> on the device. They are
      never sent to the web UI — the config editor shows a masked version.
    </UCallout>
  </div>
</template>
