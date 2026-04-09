<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Chat</h3>
    <p>
      Talk to your ZenClaw agent directly from the browser. The agent
      processes your message, calls tools as needed, and responds.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">How It Works</h4>
        <p class="text-muted">
          Type a message and press Enter. The agent receives it, decides which
          tools to use, executes them, and returns a text response. Complex
          requests may involve multiple tool calls before the final reply.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Available Tools</h4>
        <p class="text-muted">
          The agent has 40+ tools including file I/O, code execution, memory,
          cron scheduling, web search, Google Sheets, and more. Tools are
          called automatically — just ask naturally.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Sessions</h4>
        <p class="text-muted">
          Each channel (CLI, Telegram) maintains its own conversation session.
          Sessions are persisted as JSONL files and survive device reboots.
          Use <NuxtLink to="/files" class="text-primary underline">Files</NuxtLink>
          to browse session history.
        </p>
      </div>
    </div>

    <UCallout icon="i-lucide-terminal" title="Slash Commands">
      <div class="space-y-1">
        <p><code>/new</code> — start a fresh conversation (clears current session)</p>
        <p><code>/reset</code> — full reset including session file</p>
      </div>
    </UCallout>
  </div>
</template>
