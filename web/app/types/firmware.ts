export type BoardChip = 'ESP32-S3' | 'ESP32-P4'
export type BoardNetwork = 'wifi' | 'ethernet'

export interface BoardManifest {
  id: string
  name: string
  chip: BoardChip
  image: string
  app_image?: string
  network: BoardNetwork
  default?: boolean
  description?: string
}

export interface FirmwareManifest {
  boards: BoardManifest[]
}

/**
 * Hardcoded fallback used when firmware.json fails to load (offline build,
 * 404, or malformed JSON). Must stay in sync with scripts/build-rust-firmware.sh.
 */
export const FALLBACK_BOARDS: BoardManifest[] = [
  {
    id: 'devkitc',
    name: 'ESP32-S3 DevKitC',
    chip: 'ESP32-S3',
    image: 'zenclaw-devkitc.bin',
    app_image: 'zenclaw-devkitc-app.bin',
    network: 'wifi',
    default: true,
    description: '8MB PSRAM, USB Host capable',
  },
  {
    id: 'guition-p4',
    name: 'Guition JC-ESP32P4-M3-DEV',
    chip: 'ESP32-P4',
    image: 'zenclaw-guition-p4.bin',
    app_image: 'zenclaw-guition-p4-app.bin',
    network: 'ethernet',
    description: '32MB PSRAM, Ethernet via IP101 PHY',
  },
]

/**
 * Fetches firmware.json relative to the runtime baseURL. Returns the
 * fallback list on any failure (network, parse, missing fields).
 */
export async function loadBoardManifest(baseURL: string): Promise<BoardManifest[]> {
  try {
    // cache: 'no-store' — firmware.json regenerates whenever
    // scripts/build-rust-firmware.sh runs, and a stale manifest can
    // strip newly-added fields like app_image, breaking update mode.
    const resp = await fetch(baseURL + 'firmware/firmware.json', { cache: 'no-store' })
    if (!resp.ok) return FALLBACK_BOARDS
    const data = (await resp.json()) as FirmwareManifest
    if (!Array.isArray(data?.boards) || data.boards.length === 0) return FALLBACK_BOARDS
    // Validate every entry has the required fields before trusting the manifest.
    const valid = data.boards.filter(b =>
      typeof b.id === 'string' && b.id.length > 0
      && typeof b.name === 'string'
      && (b.chip === 'ESP32-S3' || b.chip === 'ESP32-P4')
      && typeof b.image === 'string'
      && (b.app_image === undefined || typeof b.app_image === 'string')
      && (b.network === 'wifi' || b.network === 'ethernet'),
    )
    return valid.length > 0 ? valid : FALLBACK_BOARDS
  } catch {
    return FALLBACK_BOARDS
  }
}
