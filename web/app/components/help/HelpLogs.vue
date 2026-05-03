<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Logs</h3>
    <p>
      Live log stream from the device, delivered over WebSocket in real time.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Log Levels</h4>
        <p class="text-muted">Each log entry has a level with a distinct color:</p>
        <ul class="mt-1 space-y-1">
          <li><span class="text-blue-500 font-medium">info</span> — general operational messages</li>
          <li><span class="text-yellow-500 font-medium">warning</span> — potential issues</li>
          <li><span class="text-red-500 font-medium">error</span> — failures and exceptions</li>
          <li><span class="text-gray-400 font-medium">debug</span> — verbose diagnostic output</li>
        </ul>
      </div>

      <div>
        <h4 class="font-medium">Filtering</h4>
        <p class="text-muted">
          Filter logs by level and source to narrow down what you see. Use the
          dropdown controls above the log stream.
        </p>
      </div>
    </div>

    <UAlert icon="i-lucide-wifi" title="Connection">
      <template #description>
        The log stream auto-reconnects if the WebSocket connection drops. If you
        need to scroll through history, pause the live stream first — new
        messages won't auto-scroll while paused.
      </template>
    </UAlert>
  </div>
</template>
