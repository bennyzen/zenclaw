import type {
  ChatEvent,
  ConnectionState,
  DeviceStatus,
  FileEntry,
} from '~/types/connection'

const state = reactive<ConnectionState>({
  mode: 'disconnected',
  serialConnected: false,
  networkConnected: false,
  connecting: false,
  deviceIp: null,
  devicePort: 8443,
  useTls: true,
  lastStatus: null,
  error: null,
})

let statsWs: WebSocket | null = null
let statsPollTimer: ReturnType<typeof setInterval> | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
let reconnectDelay = 2000
let _savedHostname: string | null = null

function updateMode() {
  const s = state.serialConnected
  const n = state.networkConnected
  if (s && n) state.mode = 'both'
  else if (s) state.mode = 'serial'
  else if (n) state.mode = 'network'
  else state.mode = 'disconnected'
}

function baseUrl(): string {
  if (state.useTls) return `https://${state.deviceIp}:${state.devicePort}`
  return `http://${state.deviceIp}`
}

function wsUrl(): string {
  if (state.useTls) return `wss://${state.deviceIp}:${state.devicePort}`
  return `ws://${state.deviceIp}`
}

async function apiFetch<T>(path: string, options: RequestInit = {}): Promise<T> {
  const url = `${baseUrl()}${path}`
  const headers: Record<string, string> = { ...options.headers as Record<string, string> }
  if (options.body) headers['Content-Type'] = 'application/json'
  const res = await fetch(url, { ...options, headers })
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }))
    const err: any = new Error(body.error || `HTTP ${res.status}`)
    err.status = res.status
    throw err
  }
  return res.json()
}

function mapStatus(raw: Record<string, any>): DeviceStatus {
  return {
    agentName: raw.agent_name ?? 'Unknown',
    version: raw.version ?? 'unknown',
    built: raw.built ?? '',
    board: raw.board && raw.board !== 'unknown' ? raw.board : null,
    platform: raw.platform ?? null,
    memory: raw.memory
      ? {
          freeKb: raw.memory.free_kb,
          usedKb: raw.memory.used_kb,
          totalKb: raw.memory.total_kb,
        }
      : null,
    temperatureC: raw.temperature_c ?? null,
    wifi: raw.wifi
      ? {
          connected: raw.wifi.connected,
          ip: raw.wifi.ip ?? null,
          rssi: raw.wifi.rssi ?? null,
        }
      : null,
    storage: raw.storage
      ? {
          totalKb: raw.storage.total_kb,
          freeKb: raw.storage.free_kb,
        }
      : null,
    cloudStorage: raw.cloud_storage
      ? {
          configured: raw.cloud_storage.configured,
          provider: raw.cloud_storage.provider,
          bucket: raw.cloud_storage.bucket,
          objects: raw.cloud_storage.objects,
          totalBytes: raw.cloud_storage.total_bytes,
          error: raw.cloud_storage.error,
        }
      : null,
    provider: raw.provider || null,
    model: raw.model || null,
    uptimeS: raw.uptime_s ?? null,
  }
}

function cancelReconnect() {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  reconnectDelay = 2000
}

function scheduleReconnect() {
  if (!_savedHostname || reconnectTimer) return
  reconnectTimer = setTimeout(async () => {
    reconnectTimer = null
    if (state.networkConnected || !_savedHostname) return
    try {
      await useConnection().connectNetwork(_savedHostname)
      reconnectDelay = 2000
    } catch {
      reconnectDelay = Math.min(reconnectDelay * 2, 30000)
      scheduleReconnect()
    }
  }, reconnectDelay)
}

// Stats transport: WebSocket is primary, GET poll is fallback when WS is
// down. The device serves the same payload on both transports (see the
// shared build_status_payload in agent/src/main.rs and
// agent/src/desktop/server.rs), so a single setStatus does a full
// replace regardless of which channel delivered it. No per-field merge,
// no flicker, no asymmetry.
//
// Spec: docs/superpowers/specs/2026-05-03-stats-transport-model.md.

function setStatus(raw: Record<string, any>) {
  state.lastStatus = mapStatus(raw)
}

let _pollFailCount = 0
let wsRetryTimer: ReturnType<typeof setTimeout> | null = null

function startStatsPoll() {
  if (statsPollTimer) return
  _pollFailCount = 0
  statsPollTimer = setInterval(async () => {
    if (!state.networkConnected) return
    // If WS came back while we were polling, drop the poll and let the
    // push handler take over.
    if (statsWs?.readyState === WebSocket.OPEN) {
      stopStatsPoll()
      return
    }
    try {
      const res = await fetch(`${baseUrl()}/api/status`, { signal: AbortSignal.timeout(10000) })
      if (res.ok) {
        setStatus(await res.json())
        _pollFailCount = 0
      } else {
        _pollFailCount++
      }
    } catch {
      _pollFailCount++
    }
    if (_pollFailCount >= 3) {
      stopStatsPoll()
      state.networkConnected = false
      updateMode()
      scheduleReconnect()
    }
  }, 15000)
}

function stopStatsPoll() {
  if (statsPollTimer) {
    clearInterval(statsPollTimer)
    statsPollTimer = null
  }
}

function ensureStatsTransport() {
  if (!state.networkConnected) return
  if (statsWs) return
  const url = `${wsUrl()}/ws/stats`
  const ws = new WebSocket(url)
  statsWs = ws
  ws.onopen = () => {
    // WS is alive — drop the GET fallback.
    stopStatsPoll()
    if (wsRetryTimer) { clearTimeout(wsRetryTimer); wsRetryTimer = null }
  }
  ws.onmessage = (event) => {
    try { setStatus(JSON.parse(event.data)) } catch { /* ignore parse errors */ }
  }
  ws.onclose = () => {
    statsWs = null
    if (!state.networkConnected) return
    // WS dropped — start polling as fallback and keep retrying the WS.
    startStatsPoll()
    if (!wsRetryTimer) {
      wsRetryTimer = setTimeout(() => {
        wsRetryTimer = null
        ensureStatsTransport()
      }, 5000)
    }
  }
  ws.onerror = () => { ws.close() }
}

function stopStatsStream() {
  stopStatsPoll()
  if (wsRetryTimer) { clearTimeout(wsRetryTimer); wsRetryTimer = null }
  if (statsWs) {
    statsWs.close()
    statsWs = null
  }
}

export function useConnection() {
  async function connectNetwork(ip: string, port: number = 8443) {
    state.error = null
    state.deviceIp = ip
    state.devicePort = port
    state.connecting = true
    _savedHostname = ip
    cancelReconnect()

    // Race HTTP and HTTPS in parallel with 30s timeout each
    const attempts = [false, true].map(async (tls): Promise<{ tls: boolean; data: Record<string, any> }> => {
      const base = tls ? `https://${ip}:${port}` : `http://${ip}`
      const res = await fetch(`${base}/api/status`, {
        signal: AbortSignal.timeout(30000),
      })
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      return { tls, data: await res.json() }
    })

    try {
      const { tls, data } = await Promise.any(attempts)
      state.useTls = tls
      state.lastStatus = mapStatus(data)
      state.networkConnected = true
      state.connecting = false
      updateMode()
      // Try the WS first; on failure it falls through to the GET poll.
      // The /api/config fill-in for model is no longer needed — both
      // transports now carry provider/model on every payload.
      ensureStatsTransport()
    } catch (e) {
      state.networkConnected = false
      state.connecting = false
      state.error = e instanceof AggregateError
        ? e.errors.map((err: Error) => err.message).join('; ')
        : 'Connection failed'
      updateMode()
      throw new Error(state.error)
    }
  }

  function disconnectNetwork() {
    cancelReconnect()
    _savedHostname = null
    state.networkConnected = false
    stopStatsStream()
    state.connecting = false
    state.deviceIp = null
    state.lastStatus = null
    state.error = null
    updateMode()
  }

  // File operations
  async function listDir(path: string): Promise<{ path: string; entries: FileEntry[] }> {
    const raw = await apiFetch<{ path: string; entries: any[] }>(
      `/api/files?path=${encodeURIComponent(path)}`
    )
    return {
      path: raw.path,
      entries: raw.entries.map((e: any) => ({
        name: e.name,
        path: e.path,
        isDir: e.is_dir,
        size: e.size ?? null,
      })),
    }
  }

  async function readFile(path: string): Promise<{ path: string; content: string }> {
    return apiFetch(`/api/files/read?path=${encodeURIComponent(path)}`)
  }

  async function writeFile(path: string, content: string): Promise<{ path: string; size: number }> {
    return apiFetch('/api/files/write', {
      method: 'PUT',
      body: JSON.stringify({ path, content }),
    })
  }

  async function deleteFile(path: string): Promise<{ deleted: string }> {
    return apiFetch(`/api/files?path=${encodeURIComponent(path)}`, {
      method: 'DELETE',
    })
  }

  async function createDir(path: string): Promise<{ path: string }> {
    return apiFetch('/api/files/mkdir', {
      method: 'POST',
      body: JSON.stringify({ path }),
    })
  }

  async function uploadFile(path: string, data: ArrayBuffer): Promise<{ path: string; size: number }> {
    const url = `${baseUrl()}/api/files/upload?path=${encodeURIComponent(path)}`
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: new Blob([data]),
    })
    if (!res.ok) {
      const body = await res.json().catch(() => ({ error: res.statusText }))
      throw new Error(body.error || `HTTP ${res.status}`)
    }
    return res.json()
  }

  // Cloud storage operations (presigned URLs — browser talks directly to R2)
  async function listCloudDir(prefix: string): Promise<{ path: string; entries: FileEntry[] }> {
    const raw = await apiFetch<{ prefix: string; entries: any[] }>(
      `/api/cloud/files?prefix=${encodeURIComponent(prefix)}`,
    )
    return {
      path: raw.prefix,
      entries: raw.entries.map((e: any) => ({
        name: e.name,
        path: e.path,
        isDir: e.is_dir,
        size: e.size ?? null,
      })),
    }
  }

  async function cloudSign(method: string, key: string, contentType?: string): Promise<string> {
    let url = `/api/cloud/sign?method=${method}&key=${encodeURIComponent(key)}`
    if (contentType) url += `&content_type=${encodeURIComponent(contentType)}`
    const res = await apiFetch<{ url: string }>(url)
    return res.url
  }

  async function readCloudFile(key: string): Promise<{ path: string; content: string }> {
    const url = await cloudSign('GET', key)
    const res = await fetch(url)
    if (!res.ok) throw new Error(`Cloud read failed: HTTP ${res.status}`)
    return { path: key, content: await res.text() }
  }

  async function writeCloudFile(key: string, content: string): Promise<void> {
    const ct = key.endsWith('.json') ? 'application/json' : 'text/plain; charset=utf-8'
    const url = await cloudSign('PUT', key, ct)
    const res = await fetch(url, { method: 'PUT', body: content, headers: { 'Content-Type': ct } })
    if (!res.ok) throw new Error(`Cloud write failed: HTTP ${res.status}`)
  }

  async function deleteCloudFile(key: string): Promise<void> {
    const url = await cloudSign('DELETE', key)
    const res = await fetch(url, { method: 'DELETE' })
    if (!res.ok) throw new Error(`Cloud delete failed: HTTP ${res.status}`)
  }

  async function uploadCloudFile(key: string, data: ArrayBuffer): Promise<void> {
    const url = await cloudSign('PUT', key, 'application/octet-stream')
    const res = await fetch(url, { method: 'PUT', body: new Blob([data]) })
    if (!res.ok) throw new Error(`Cloud upload failed: HTTP ${res.status}`)
  }

  // Config operations
  async function getConfig(): Promise<Record<string, any>> {
    return apiFetch('/api/config')
  }

  async function saveConfig(config: Record<string, any>): Promise<{ ok: boolean }> {
    return apiFetch('/api/config', {
      method: 'PUT',
      body: JSON.stringify(config),
    })
  }

  // WiFi operations
  async function getWifi(): Promise<{ ssid: string | null; connected: boolean; ip: string | null; rssi: number | null }> {
    return apiFetch('/api/wifi')
  }

  async function setWifi(ssid: string, password: string): Promise<{ ok: boolean; connected: boolean; ip: string | null }> {
    return apiFetch('/api/wifi', {
      method: 'PUT',
      body: JSON.stringify({ ssid, password }),
    })
  }

  // Chat operations
  async function sendChat(message: string, chatId = 'web'): Promise<{ reply: string }> {
    return apiFetch('/api/chat', {
      method: 'POST',
      body: JSON.stringify({ message, chat_id: chatId }),
    })
  }

  /// Open a streaming chat session over WS. Returns a handle the caller uses
  /// to send messages, cancel turns, and close. `onEvent` fires for every
  /// typed `ChatEvent` from the server (thinking, tool_call_*, assistant_text,
  /// done, error). The browser-originated `user_message` and `cancel` events
  /// are sent via `send`/`cancel`; the server never echoes them back.
  function openChatStream(
    onEvent: (evt: ChatEvent) => void,
    chatId = 'web',
  ): {
    send: (text: string) => void
    cancel: () => void
    close: () => void
    isOpen: () => boolean
  } {
    const url = `${wsUrl()}/ws/chat`
    let ws: WebSocket | null = new WebSocket(url)
    const queue: string[] = []
    let opened = false

    ws.onopen = () => {
      opened = true
      while (queue.length) {
        ws?.send(queue.shift()!)
      }
    }
    ws.onmessage = (event) => {
      try {
        const evt = JSON.parse(event.data) as ChatEvent
        onEvent(evt)
      } catch { /* ignore non-JSON frames */ }
    }
    ws.onerror = () => {
      onEvent({ type: 'error', error: 'WebSocket connection failed' })
    }
    ws.onclose = () => { ws = null }

    function transmit(payload: string) {
      if (!ws) return
      if (opened) ws.send(payload)
      else queue.push(payload)
    }

    return {
      send: (text: string) => {
        transmit(JSON.stringify({ type: 'user_message', chat_id: chatId, text }))
      },
      cancel: () => {
        transmit(JSON.stringify({ type: 'cancel', chat_id: chatId }))
      },
      close: () => {
        ws?.close()
        ws = null
      },
      isOpen: () => ws !== null,
    }
  }

  async function getChatHistory(chatId = 'web', limit = 200): Promise<{ events: ChatEvent[] }> {
    return apiFetch(`/api/chat/history?chat_id=${encodeURIComponent(chatId)}&limit=${limit}`)
  }

  // Device operations
  async function restartDevice(): Promise<void> {
    await apiFetch('/api/restart', { method: 'POST' })
  }

  return {
    state: readonly(state) as Readonly<ConnectionState>,
    connectNetwork,
    disconnectNetwork,
    listDir,
    readFile,
    writeFile,
    deleteFile,
    createDir,
    uploadFile,
    listCloudDir,
    readCloudFile,
    writeCloudFile,
    deleteCloudFile,
    uploadCloudFile,
    getConfig,
    saveConfig,
    getWifi,
    setWifi,
    sendChat,
    openChatStream,
    getChatHistory,
    restartDevice,
    wsUrl,
  }
}
