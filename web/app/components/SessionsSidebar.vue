<script setup lang="ts">
const route = useRoute()
const router = useRouter()

const { sessions, loading, error, refresh, create, rename, remove } = useSessions()

const query = ref('')
const renamingId = ref<string | null>(null)
const renameDraft = ref('')
const renameInput = ref<HTMLInputElement | null>(null)
const confirmOpen = ref(false)
const pendingDelete = ref<string | null>(null)

// Initial fetch (refresh handles disconnected state internally).
onMounted(() => refresh())

const filtered = computed(() => {
  if (!query.value.trim()) return sessions.value
  const q = query.value.trim().toLowerCase()
  return sessions.value.filter((s) => s.title.toLowerCase().includes(q))
})

function kindIcon(k: string) {
  return ({
    web: 'i-lucide-message-circle',
    telegram: 'i-lucide-send',
    cron: 'i-lucide-clock',
    other: 'i-lucide-circle',
  } as Record<string, string>)[k] || 'i-lucide-circle'
}

function relative(ms: number) {
  const diff = Date.now() - ms
  const min = Math.round(diff / 60_000)
  if (min < 1) return 'now'
  if (min < 60) return `${min}m`
  const hr = Math.round(min / 60)
  if (hr < 24) return `${hr}h`
  const d = Math.round(hr / 24)
  return `${d}d`
}

async function onNewChat() {
  try {
    const meta = await create()
    router.push(`/chat/${encodeURIComponent(meta.chatId)}`)
  } catch (e: any) {
    console.error('Create failed:', e?.message || e)
  }
}

function rowMenu(session: { chatId: string; title: string }) {
  return [[
    {
      label: 'Rename',
      icon: 'i-lucide-pencil',
      onSelect: () => beginRename(session.chatId, session.title),
    },
    {
      label: 'Delete',
      icon: 'i-lucide-trash',
      color: 'error' as const,
      onSelect: () => {
        pendingDelete.value = session.chatId
        confirmOpen.value = true
      },
    },
  ]]
}

async function beginRename(id: string, current: string) {
  renamingId.value = id
  renameDraft.value = current
  await nextTick()
  renameInput.value?.focus()
  renameInput.value?.select()
}

async function commitRename(id: string) {
  const newTitle = renameDraft.value.trim()
  renamingId.value = null
  if (!newTitle) return
  try {
    await rename(id, newTitle)
  } catch (e: any) {
    console.error('Rename failed:', e?.message || e)
  }
}

async function confirmDelete() {
  const id = pendingDelete.value
  confirmOpen.value = false
  pendingDelete.value = null
  if (!id) return
  try {
    if (route.params.id === id) router.push('/chat')
    await remove(id)
  } catch (e: any) {
    console.error('Delete failed:', e?.message || e)
  }
}
</script>

<template>
  <aside class="flex flex-col h-full w-[300px] border-r border-default bg-elevated">
    <div class="p-3 space-y-2 border-b border-default">
      <UButton block color="primary" icon="i-lucide-plus" @click="onNewChat">
        New chat
      </UButton>
      <UInput
        v-model="query"
        placeholder="Search conversations..."
        icon="i-lucide-search"
        size="sm"
      />
    </div>

    <div v-if="error" class="m-3 p-2 text-sm text-error border border-error rounded">
      <p>{{ error }}</p>
      <UButton size="xs" variant="ghost" @click="refresh">Retry</UButton>
    </div>

    <div class="flex-1 overflow-y-auto">
      <div v-if="filtered.length === 0 && !loading" class="p-4 text-sm text-muted">
        <template v-if="query">No conversations match "{{ query }}".</template>
        <template v-else>No conversations yet — click "New chat" to start.</template>
      </div>

      <NuxtLink
        v-for="session in filtered"
        :key="session.chatId"
        :to="`/chat/${encodeURIComponent(session.chatId)}`"
        class="block px-3 py-2 border-b border-default hover:bg-accented"
        :class="{ 'bg-accented': route.params.id === session.chatId }"
      >
        <div class="flex items-center gap-2">
          <UIcon :name="kindIcon(session.kind)" class="text-muted shrink-0" />
          <input
            v-if="renamingId === session.chatId"
            ref="renameInput"
            v-model="renameDraft"
            class="flex-1 bg-transparent border-b border-primary outline-none text-sm"
            @blur="commitRename(session.chatId)"
            @keyup.enter="commitRename(session.chatId)"
            @keyup.escape="renamingId = null"
          />
          <span v-else class="flex-1 truncate font-medium text-sm">{{ session.title }}</span>
          <span class="text-xs text-muted shrink-0">{{ relative(session.lastActivityMs) }}</span>
          <UDropdownMenu :items="rowMenu(session)" :content="{ align: 'end' }">
            <UButton
              icon="i-lucide-ellipsis-vertical"
              variant="ghost"
              color="neutral"
              size="xs"
              @click.prevent
            />
          </UDropdownMenu>
        </div>
        <p v-if="session.lastMessagePreview" class="text-xs text-muted truncate mt-0.5 ml-6">
          {{ session.lastMessagePreview }}
        </p>
      </NuxtLink>
    </div>

    <UModal v-model:open="confirmOpen">
      <template #content>
        <div class="p-4 space-y-3">
          <h3 class="font-semibold">Delete this conversation?</h3>
          <p class="text-sm text-muted">This cannot be undone.</p>
          <div class="flex justify-end gap-2">
            <UButton variant="ghost" @click="confirmOpen = false">Cancel</UButton>
            <UButton color="error" @click="confirmDelete">Delete</UButton>
          </div>
        </div>
      </template>
    </UModal>
  </aside>
</template>
