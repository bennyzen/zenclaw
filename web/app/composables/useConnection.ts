import type {
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
    throw new Error(body.error || `HTTP ${res.status}`)
  }
  return res.json()
}

function mapStatus(raw: Record<string, any>): DeviceStatus {
  return {
    agentName: raw.agent_name ?? 'Unknown',
    version: raw.version ?? 'unknown',
    built: raw.built ?? '',
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
    cloudStorage: raw.cloud_storage ?? null,
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

function startStatsStream() {
  if (statsWs) return
  const url = `${wsUrl()}/ws/stats`
  statsWs = new WebSocket(url)
  statsWs.onmessage = (event) => {
    try {
      const raw = JSON.parse(event.data)
      // Stats stream doesn't include agent_name/version, merge with existing
      const partial = mapStatus(raw)
      if (state.lastStatus) {
        state.lastStatus = {
          ...state.lastStatus,
          memory: partial.memory,
          temperatureC: partial.temperatureC,
          wifi: partial.wifi,
          storage: partial.storage,
          uptimeS: partial.uptimeS,
        }
      }
    } catch { /* ignore parse errors */ }
  }
  statsWs.onclose = () => {
    statsWs = null
    if (state.networkConnected) {
      state.networkConnected = false
      updateMode()
      scheduleReconnect()
    }
  }
  statsWs.onerror = () => { statsWs?.close() }
}

function stopStatsStream() {
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
      startStatsStream()
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

  function sendChatStream(
    message: string,
    onDelta: (text: string) => void,
    onDone: (fullText: string) => void,
    onError: (error: string) => void,
    chatId = 'web',
  ) {
    const url = `${wsUrl()}/ws/chat`
    const ws = new WebSocket(url)
    ws.onopen = () => {
      ws.send(JSON.stringify({ message, chat_id: chatId }))
    }
    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data)
        if (msg.type === 'delta') onDelta(msg.text)
        else if (msg.type === 'done') { onDone(msg.text); ws.close() }
        else if (msg.type === 'error') { onError(msg.error); ws.close() }
      } catch { /* ignore */ }
    }
    ws.onerror = () => { onError('WebSocket connection failed'); ws.close() }
    return ws
  }

  async function getChatHistory(chatId = 'web', limit = 50): Promise<{ messages: { role: string; content: string }[] }> {
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
    sendChatStream,
    getChatHistory,
    restartDevice,
    wsUrl,
  }
}
