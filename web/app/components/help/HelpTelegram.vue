<template>
  <div class="space-y-4 text-sm">
    <h3 class="text-base font-semibold">Telegram Bot Setup</h3>
    <p>
      Connect your ZenClaw device to Telegram for messaging from anywhere.
    </p>

    <div class="space-y-3">
      <div>
        <h4 class="font-medium">Step 1 — Create a Bot</h4>
        <p class="text-muted">
          Open Telegram and message
          <a href="https://t.me/BotFather" target="_blank" class="text-primary underline">@BotFather</a>.
          Send <code>/newbot</code>, follow the prompts to choose a name and
          username, then copy the bot token.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Step 2 — Start a Conversation</h4>
        <p class="text-muted">
          Open your new bot and tap <strong>Start</strong> (or send any
          message). This creates a chat that the bot can reply to.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Step 3 — Get Your Chat ID</h4>
        <p class="text-muted">
          Visit this URL in your browser, replacing <code>TOKEN</code> with
          your bot token:
        </p>
        <code class="block mt-1 text-xs break-all">https://api.telegram.org/botTOKEN/getUpdates</code>
        <p class="text-muted mt-1">
          Look for <code>"chat":{"id": 123456789}</code> in the JSON response.
          That number is your chat ID.
        </p>
      </div>

      <div>
        <h4 class="font-medium">Step 4 — Configure</h4>
        <p class="text-muted">
          Paste the bot token and chat ID into the Telegram section of the
          <NuxtLink to="/config" class="text-primary underline">Config</NuxtLink>
          page, then enable the Telegram channel.
        </p>
      </div>
    </div>

    <UCallout icon="i-lucide-users" title="Group Chats">
      <p>To use in a group:</p>
      <ol class="list-decimal list-inside mt-1 space-y-1">
        <li>Add the bot to the group</li>
        <li>Get the group chat ID (it will be a negative number)</li>
        <li>Add it to <code>allowed_chat_ids</code> in config</li>
      </ol>
      <p class="mt-2">
        DMs use direct message delivery. Group chats use edit-based streaming
        for real-time responses.
      </p>
    </UCallout>

    <UCallout icon="i-lucide-shield" title="Privacy">
      Use <code>allowed_chat_ids</code> to restrict which chats can interact
      with the bot. Without it, anyone who finds your bot can send messages.
    </UCallout>
  </div>
</template>
