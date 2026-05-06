export type ConnectionMode = 'disconnected' | 'serial' | 'network' | 'both'

export interface DeviceStatus {
  agentName: string
  version: string
  built: string
  board: string | null
  platform: string | null
  memory: { freeKb: number; usedKb: number; totalKb: number } | null
  temperatureC: number | null
  wifi: { connected: boolean; ip: string | null; rssi: number | null } | null
  storage: { totalKb: number; freeKb: number } | null
  cloudStorage: { configured: boolean; provider?: string; bucket?: string; objects?: number; totalBytes?: number; error?: string } | null
  sdcard: { mounted: boolean; path?: string; totalKb?: number; freeKb?: number; type?: string; busWidth?: number } | null
  provider: string | null
  model: string | null
  uptimeS: number | null
}

export interface FileEntry {
  name: string
  path: string
  isDir: boolean
  size: number | null
}

export interface ConnectionState {
  mode: ConnectionMode
  serialConnected: boolean
  networkConnected: boolean
  connecting: boolean
  deviceIp: string | null
  devicePort: number
  useTls: boolean
  lastStatus: DeviceStatus | null
  error: string | null
}

// ---------------------------------------------------------------------------
// Session metadata (mirror of agent/src/core/sessions/mod.rs SessionMeta)
// ---------------------------------------------------------------------------

export interface SessionMeta {
  chatId: string
  kind: 'web' | 'telegram' | 'cron' | 'other'
  title: string
  titleSource: 'llm' | 'user' | 'firstMessage' | 'default'
  createdAtMs: number
  lastActivityMs: number
  lastMessagePreview: string
  version: number
}

// ---------------------------------------------------------------------------
// Chat events (mirror of agent/src/core/chat_events.rs)
// ---------------------------------------------------------------------------

export type ChatEvent =
  | { type: 'user_message'; chat_id: string; text: string }
  | { type: 'thinking_started' }
  | { type: 'thinking_ended' }
  | { type: 'tool_call_started'; id: string; name: string; args: unknown }
  | { type: 'tool_call_finished'; id: string; ok: boolean; result?: string; error?: string }
  | { type: 'assistant_text'; text: string; final: boolean }
  | { type: 'done' }
  | { type: 'error'; error: string }
  | { type: 'cancel'; chat_id: string }
