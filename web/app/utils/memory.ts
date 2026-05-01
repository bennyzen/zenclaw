/**
 * ZenClaw memory format — a single `data/MEMORY.md` file. Mirrors the on-device
 * parser in `agent/src/core/tools/memory_tools.rs`. Supports both the current
 * format (title on `##` heading, metadata below) and the legacy format
 * (metadata fused into the heading; no title).
 *
 * Current:
 *
 *     ## Prefers explicit error handling
 *     [mem_a3f2c1d8] 2026-05-01T10:30:00Z (tags: preference)
 *
 *     Hates unwrap() outside tests.
 *
 * Legacy:
 *
 *     ## [mem_a3f2c1d8] 2026-05-01T10:30:00Z (tags: preference)
 *     Body content.
 */

export interface MemoryBlock {
  id: string
  timestamp: string
  /** Short label, ≤ MAX_TITLE_CHARS. Empty for legacy entries. */
  title: string
  tags: string[]
  content: string
}

/** Hard caps — must match the agent constants in memory_tools.rs. */
export const MAX_BYTES = 64 * 1024
export const MAX_ENTRIES = 200
export const WARN_THRESHOLD_PCT = 70
export const MAX_TITLE_CHARS = 80
export const LEGACY_TITLE_PREVIEW_CHARS = 60

interface ParsedMetadata {
  id: string
  timestamp: string
  tags: string[]
}

/** Parse the on-disk MEMORY.md format. Accepts both layouts. */
export function parseMemoryFile(text: string): MemoryBlock[] {
  const lines = text.split(/\r?\n/)
  const blocks: MemoryBlock[] = []
  let i = 0

  while (i < lines.length) {
    const headerHit = tryParseHeaderAt(lines, i)
    if (headerHit) {
      const { block, bodyStart } = headerHit
      const bodyEnd = nextBlockStart(lines, bodyStart)
      block.content = lines.slice(bodyStart, bodyEnd).join('\n').trim()
      blocks.push(block)
      i = bodyEnd
    } else {
      i++
    }
  }
  return blocks
}

function tryParseHeaderAt(lines: string[], i: number): { block: MemoryBlock; bodyStart: number } | null {
  const line = lines[i]
  if (!line) return null
  const trimmed = line.trimStart()
  if (!trimmed.startsWith('## ')) return null
  const rest = trimmed.slice(3).trim()

  // Legacy: heading carries the metadata.
  if (rest.startsWith('[')) {
    const meta = parseMetadata(rest)
    if (!meta) return null
    return {
      block: { id: meta.id, timestamp: meta.timestamp, title: '', tags: meta.tags, content: '' },
      bodyStart: i + 1,
    }
  }

  // Current: heading is title; next non-blank line is metadata.
  let j = i + 1
  while (j < lines.length && (lines[j] ?? '').trim() === '') j++
  if (j >= lines.length) return null
  const meta = parseMetadata((lines[j] ?? '').trim())
  if (!meta) return null
  return {
    block: { id: meta.id, timestamp: meta.timestamp, title: rest, tags: meta.tags, content: '' },
    bodyStart: j + 1,
  }
}

function nextBlockStart(lines: string[], start: number): number {
  let i = start
  while (i < lines.length) {
    if (tryParseHeaderAt(lines, i)) return i
    i++
  }
  return lines.length
}

function parseMetadata(s: string): ParsedMetadata | null {
  const trimmed = s.trim()
  if (!trimmed.startsWith('[')) return null
  const closeBracket = trimmed.indexOf(']')
  if (closeBracket === -1) return null
  const id = trimmed.slice(1, closeBracket).trim()
  const rest = trimmed.slice(closeBracket + 1).trimStart()

  const tagsIdx = rest.indexOf(' (tags:')
  let timestamp = rest
  let tags: string[] = []
  if (tagsIdx !== -1) {
    timestamp = rest.slice(0, tagsIdx).trim()
    const tagsPart = rest.slice(tagsIdx + ' (tags:'.length).replace(/\)\s*$/, '').trim()
    tags = parseTags(tagsPart)
  } else {
    timestamp = rest.trim()
  }

  return { id, timestamp, tags }
}

/**
 * Serialize blocks. Entries with a title use the current layout; entries
 * without one (legacy, never edited) round-trip in legacy format so the
 * on-disk shape doesn't churn for untouched data.
 */
export function serializeMemoryFile(blocks: MemoryBlock[]): string {
  const out: string[] = []
  blocks.forEach((b, i) => {
    if (i > 0) out.push('')
    if (b.title) {
      out.push(`## ${b.title}`)
      out.push(metadataLine(b))
      out.push('')
      out.push(b.content)
    } else {
      out.push(`## ${metadataLine(b)}`)
      out.push(b.content)
    }
  })
  return out.length > 0 ? out.join('\n') + '\n' : ''
}

function metadataLine(b: MemoryBlock): string {
  const base = `[${b.id}] ${b.timestamp}`
  return b.tags.length > 0 ? `${base} (tags: ${b.tags.join(', ')})` : base
}

/**
 * What to display as the title in cards/lists. Prefers explicit title; for
 * legacy entries derives a quoted snippet from the first line of content.
 */
export function displayTitle(b: MemoryBlock): string {
  if (b.title) return b.title
  const firstLine = (b.content.split(/\r?\n/, 1)[0] ?? '').trim()
  if (!firstLine) return '(empty)'
  if ([...firstLine].length <= LEGACY_TITLE_PREVIEW_CHARS) return `"${firstLine}"`
  const truncated = [...firstLine].slice(0, LEGACY_TITLE_PREVIEW_CHARS).join('').trimEnd()
  return `"${truncated}…"`
}

export function parseTags(s: string): string[] {
  return s
    .split(',')
    .map(t => t.trim())
    .filter(t => t.length > 0)
}

/** Generate a fresh `mem_<8 hex>` id matching the agent's format. */
export function newMemoryId(): string {
  const hex = '0123456789abcdef'
  let id = 'mem_'
  for (let i = 0; i < 8; i++) id += hex[Math.floor(Math.random() * 16)]
  return id
}

export function nowTimestamp(): string {
  return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z')
}

export interface CapacityInfo {
  bytes: number
  count: number
  bytesPct: number
  countPct: number
  pct: number
  near: boolean
  full: boolean
}

export function capacityInfo(blocks: MemoryBlock[], serialized?: string): CapacityInfo {
  const bytes = serialized ? new TextEncoder().encode(serialized).length : 0
  const count = blocks.length
  const bytesPct = Math.min(100, Math.floor((bytes * 100) / MAX_BYTES))
  const countPct = Math.min(100, Math.floor((count * 100) / MAX_ENTRIES))
  const pct = Math.max(bytesPct, countPct)
  return {
    bytes,
    count,
    bytesPct,
    countPct,
    pct,
    near: pct >= WARN_THRESHOLD_PCT,
    full: pct >= 100,
  }
}

/** Tag-frequency map across all blocks. Used for the filter pill row. */
export function tagFrequencies(blocks: MemoryBlock[]): Map<string, number> {
  const m = new Map<string, number>()
  for (const b of blocks) {
    for (const t of b.tags) {
      const k = t.toLowerCase()
      m.set(k, (m.get(k) ?? 0) + 1)
    }
  }
  return m
}

/** Friendly relative-ish timestamp ("2 days ago", "today", "Apr 30"). */
export function formatTimestamp(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  const now = Date.now()
  const diffMs = now - d.getTime()
  const day = 86_400_000
  if (diffMs < day && d.toDateString() === new Date().toDateString()) return 'today'
  if (diffMs < 2 * day) return 'yesterday'
  if (diffMs < 7 * day) return `${Math.floor(diffMs / day)} days ago`
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })
}
