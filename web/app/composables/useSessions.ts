import type { SessionMeta } from '~/types/connection'

// Module-scope singleton — shared across components, mirrors the pattern
// at useConnection.ts:8 ("state" reactive object).
const sessions = ref<SessionMeta[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

let pollHandle: ReturnType<typeof setInterval> | null = null
let focusHandlerAttached = false

async function refresh() {
  const conn = useConnection()
  // Bail if not connected — there's no device to query.
  if (!conn.state.networkConnected) {
    sessions.value = []
    return
  }
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
  const snapshot = sessions.value[idx].title
  // Optimistic mutate.
  sessions.value[idx] = { ...sessions.value[idx], title }
  try {
    const conn = useConnection()
    const updated = await conn.renameSession(id, title)
    sessions.value[idx] = updated
  } catch (e) {
    // Roll back.
    sessions.value[idx] = { ...sessions.value[idx], title: snapshot }
    throw e
  }
}

async function remove(id: string) {
  const idx = sessions.value.findIndex((s) => s.chatId === id)
  if (idx < 0) return
  const snapshot = sessions.value[idx]
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
  const updated: SessionMeta = {
    ...sessions.value[idx],
    lastActivityMs: Date.now(),
    lastMessagePreview: preview.slice(0, 120),
  }
  // Move to top — sidebar sorts by lastActivityMs desc.
  sessions.value.splice(idx, 1)
  sessions.value.unshift(updated)
}

export function useSessions() {
  // Auto-refresh hooks: attach lazily on first composable call.
  if (!focusHandlerAttached && typeof window !== 'undefined') {
    focusHandlerAttached = true
    window.addEventListener('focus', refresh)
    pollHandle = setInterval(refresh, 30_000)
  }
  return { sessions, loading, error, refresh, create, rename, remove, bumpLocal }
}
