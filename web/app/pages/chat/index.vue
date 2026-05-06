<script setup lang="ts">
definePageMeta({ layout: 'chat' })

const router = useRouter()
const { sessions, refresh } = useSessions()

// Initial fetch — refresh handles disconnected state internally.
await refresh()

onMounted(async () => {
  // Wait one tick for sessions to settle, then redirect if any exist.
  await nextTick()
  if (sessions.value.length > 0) {
    const mostRecent = [...sessions.value].sort(
      (a, b) => b.lastActivityMs - a.lastActivityMs,
    )[0]
    if (mostRecent) {
      router.replace(`/chat/${encodeURIComponent(mostRecent.chatId)}`)
    }
  }
})
</script>

<template>
  <div class="flex items-center justify-center h-full p-8 text-muted">
    <div class="text-center space-y-2">
      <UIcon name="i-lucide-message-circle" class="text-4xl text-dimmed" />
      <p>Select a chat from the sidebar or click "New chat" to start.</p>
    </div>
  </div>
</template>
