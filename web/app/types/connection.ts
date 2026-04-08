export type ConnectionMode = 'disconnected' | 'serial' | 'network' | 'both'

export interface DeviceStatus {
  agentName: string
  version: string
  built: string
  memory: { freeKb: number; usedKb: number; totalKb: number } | null
  temperatureC: number | null
  wifi: { connected: boolean; ip: string | null; rssi: number | null } | null
  storage: { totalKb: number; freeKb: number } | null
  cloudStorage: { configured: boolean; provider?: string; bucket?: string; objects?: number; totalBytes?: number; error?: string } | null
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
