import type { SessionMeta } from '~/types/connection'

// Module-scope singleton — shared across components, mirrors the pattern
// at useConnection.ts:8 ("state" reactive object).
const sessions = ref<SessionMeta[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

let focusHandlerAttached = false

async function refresh() {
  const conn = useConnection()
  // Bail if not connected — keep whatever's locally optimistic so newly
  // created chats don't blink during the disconnected → connected window.
  // (Page reloads reset reactive state anyway; this only matters for the
  // brief race between auto-connect and the first refresh.)
  if (!conn.state.networkConnected) return
  loading.value = true
  error.value = null
  try {
    const list = await conn.listSessions()
    sessions.value = list
  } catch (e: any) {
    error.value = e?.message || 'Failed to load conversations'
  } finally {
    loading.value = false
  }
}

async function create(): Promise<SessionMeta> {
  const conn = useConnection()
  const res = await conn.createSession()
  // Optimistic prepend so the new chat appears at the top immediately.
  sessions.value = [res.meta, ...sessions.value]
  return res.meta
}

async function rename(id: string, title: string) {
  const idx = sessions.value.findIndex((s) => s.chatId === id)
  if (idx < 0) return
  const current = sessions.value[idx]
  if (!current) return // type-narrowing guard for noUncheckedIndexedAccess
  const snapshot = current.title
  // Optimistic mutate.
  sessions.value[idx] = { ...current, title }
  try {
    const conn = useConnection()
    const updated = await conn.renameSession(id, title)
    sessions.value[idx] = updated
  } catch (e) {
    // Roll back — read again because the try may have mutated; restore snapshot title.
    const after = sessions.value[idx]
    if (after) {
      sessions.value[idx] = { ...after, title: snapshot }
    }
    throw e
  }
}

async function remove(id: string) {
  const idx = sessions.value.findIndex((s) => s.chatId === id)
  if (idx < 0) return
  const snapshot = sessions.value[idx]
  if (!snapshot) return // type-narrowing guard for noUncheckedIndexedAccess
  sessions.value.splice(idx, 1)
  try {
    const conn = useConnection()
    await conn.deleteSession(id)
  } catch (e) {
    sessions.value.splice(idx, 0, snapshot)
    throw e
  }
}

function bumpLocal(id: string, preview: string) {
  const idx = sessions.value.findIndex((s) => s.chatId === id)
  if (idx < 0) return
  const current = sessions.value[idx]
  if (!current) return // type-narrowing guard for noUncheckedIndexedAccess
  const updated: SessionMeta = {
    ...current,
    lastActivityMs: Date.now(),
    lastMessagePreview: preview.slice(0, 120),
  }
  // Move to top — sidebar sorts by lastActivityMs desc.
  sessions.value.splice(idx, 1)
  sessions.value.unshift(updated)
}

export function useSessions() {
  // Auto-refresh hooks: attach lazily on first composable call.
  // - Window focus: fetch when the user comes back to the tab.
  // - Connection state watcher: fetch once when the device comes online
  //   (covers the auto-connect-after-mount race).
  // No periodic polling — sessions only change in response to user actions
  // (create/rename/delete in this UI) or external channels (Telegram), and
  // the latter shouldn't make the ESP32 serve a request every 30s.
  if (!focusHandlerAttached && typeof window !== 'undefined') {
    focusHandlerAttached = true
    window.addEventListener('focus', refresh)
    const conn = useConnection()
    watch(() => conn.state.networkConnected, (connected) => {
      if (connected) refresh()
    })
  }
  return { sessions, loading, error, refresh, create, rename, remove, bumpLocal }
}
