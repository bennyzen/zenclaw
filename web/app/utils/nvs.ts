/**
 * Minimal ESP32 NVS partition binary generator.
 * Produces a binary image compatible with ESP-IDF's nvs_partition_gen.py.
 * Uses chunked blob format (BLOB_DATA 0x42 + BLOB_IDX 0x48).
 *
 * Verified against: nvs_partition_gen.py v4.4.8 with file,binary encoding.
 */

const PAGE_SIZE = 4096
const ENTRY_SIZE = 32
const HEADER_SIZE = 32
const BITMAP_SIZE = 32
const ENTRIES_OFFSET = HEADER_SIZE + BITMAP_SIZE

const PAGE_ACTIVE = 0xFFFFFFFE
const NVS_VERSION = 0xFE

const TYPE_NAMESPACE = 0x01
const TYPE_BLOB_DATA = 0x42
const TYPE_BLOB_IDX = 0x48

const crc32Table = new Uint32Array(256)
for (let i = 0; i < 256; i++) {
  let c = i
  for (let j = 0; j < 8; j++) {
    c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1)
  }
  crc32Table[i] = c
}

function crc32(data: Uint8Array): number {
  // ESP-IDF's esp_rom_crc32_le(0xFFFFFFFF, ...) pre-XORs the seed internally,
  // so the effective starting accumulator is ~0xFFFFFFFF = 0x00000000.
  let crc = 0x00000000
  for (let i = 0; i < data.length; i++) {
    crc = crc32Table[(crc ^ data[i]!) & 0xFF]! ^ (crc >>> 8)
  }
  return (crc ^ 0xFFFFFFFF) >>> 0
}

function setU32(buf: Uint8Array, offset: number, val: number) {
  buf[offset] = val & 0xFF
  buf[offset + 1] = (val >> 8) & 0xFF
  buf[offset + 2] = (val >> 16) & 0xFF
  buf[offset + 3] = (val >> 24) & 0xFF
}

function setU16(buf: Uint8Array, offset: number, val: number) {
  buf[offset] = val & 0xFF
  buf[offset + 1] = (val >> 8) & 0xFF
}

function markWritten(bitmap: Uint8Array, entryIdx: number) {
  const byteIdx = Math.floor((entryIdx * 2) / 8)
  const bitIdx = (entryIdx * 2) % 8
  bitmap[byteIdx]! &= ~(1 << bitIdx)
}

function writeEntry(page: Uint8Array, idx: number, data: Uint8Array) {
  page.set(data, ENTRIES_OFFSET + idx * ENTRY_SIZE)
}

function calcEntryCrc(entry: Uint8Array): number {
  const buf = new Uint8Array(28)
  buf.set(entry.subarray(0, 4), 0)
  buf.set(entry.subarray(8, 32), 4)
  return crc32(buf)
}

function setKey(entry: Uint8Array, key: string) {
  // Key field is 16 bytes, zero-padded (not 0xFF)
  for (let i = 8; i < 24; i++) entry[i] = 0
  const keyBytes = new TextEncoder().encode(key)
  entry.set(keyBytes.subarray(0, 15), 8)
}

export interface NvsBlob {
  namespace: string
  key: string
  value: string
}

export function buildNvsPartition(entries: NvsBlob[], partitionSize = 0x6000): Uint8Array {
  const partition = new Uint8Array(partitionSize)
  partition.fill(0xFF)

  const page = partition.subarray(0, PAGE_SIZE)

  // Page header
  setU32(page, 0, PAGE_ACTIVE)
  setU32(page, 4, 0) // sequence number
  page[8] = NVS_VERSION
  // bytes 9-27 stay 0xFF
  setU32(page, 28, crc32(page.subarray(4, 28)))

  const bitmap = page.subarray(HEADER_SIZE, HEADER_SIZE + BITMAP_SIZE)
  // Already 0xFF = all empty

  let idx = 0
  const nsMap = new Map<string, number>()
  let nextNs = 1

  // Write entries grouped by namespace (namespace decl then its blobs)
  for (const e of entries) {
    // Emit namespace entry on first encounter
    if (!nsMap.has(e.namespace)) {
      const nsIdx = nextNs++
      nsMap.set(e.namespace, nsIdx)

      const nsEntry = new Uint8Array(ENTRY_SIZE)
      nsEntry.fill(0xFF)
      nsEntry[0] = 0              // ns_index = 0 for namespace declaration
      nsEntry[1] = TYPE_NAMESPACE // 0x01
      nsEntry[2] = 1              // span
      nsEntry[3] = 0xFF           // chunk_index
      setKey(nsEntry, e.namespace)
      nsEntry[24] = nsIdx
      setU32(nsEntry, 4, calcEntryCrc(nsEntry))
      writeEntry(page, idx, nsEntry)
      markWritten(bitmap, idx)
      idx++
    }

    const nsIdx = nsMap.get(e.namespace)!
    const data = new TextEncoder().encode(e.value)
    const dataEntries = Math.ceil(data.length / ENTRY_SIZE)
    const dataSpan = 1 + dataEntries

    // --- BLOB_DATA (0x42) ---
    const blobData = new Uint8Array(ENTRY_SIZE)
    blobData.fill(0xFF)
    blobData[0] = nsIdx
    blobData[1] = TYPE_BLOB_DATA
    blobData[2] = dataSpan
    blobData[3] = 0 // chunk_index = 0
    setKey(blobData, e.key)
    setU16(blobData, 24, data.length)
    // bytes 26-27 stay 0xFF (reserved)

    const dataArea = new Uint8Array(dataEntries * ENTRY_SIZE)
    dataArea.fill(0xFF)
    dataArea.set(data, 0)
    setU32(blobData, 28, crc32(data))
    setU32(blobData, 4, calcEntryCrc(blobData))

    writeEntry(page, idx, blobData)
    markWritten(bitmap, idx)
    idx++

    for (let i = 0; i < dataEntries; i++) {
      writeEntry(page, idx, dataArea.subarray(i * ENTRY_SIZE, (i + 1) * ENTRY_SIZE))
      markWritten(bitmap, idx)
      idx++
    }

    // --- BLOB_IDX (0x48) ---
    const blobIdx = new Uint8Array(ENTRY_SIZE)
    blobIdx.fill(0xFF)
    blobIdx[0] = nsIdx
    blobIdx[1] = TYPE_BLOB_IDX
    blobIdx[2] = 1     // span = 1
    blobIdx[3] = 0xFF  // chunk_index
    setKey(blobIdx, e.key)
    setU32(blobIdx, 24, data.length) // total size
    setU16(blobIdx, 28, 1)           // chunk_count = 1
    // bytes 30-31 stay 0xFF
    setU32(blobIdx, 4, calcEntryCrc(blobIdx))

    writeEntry(page, idx, blobIdx)
    markWritten(bitmap, idx)
    idx++
  }

  return partition
}
