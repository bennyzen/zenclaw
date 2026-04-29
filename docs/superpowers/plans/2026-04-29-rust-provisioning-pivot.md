# Rust Provisioning Pivot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pivot the web UI flash wizard from MicroPython to the Rust agent, supporting all three Rust target boards (DevKitC, T-Dongle-S3, Guition P4), hiding WiFi for Ethernet-only boards, and giving each device a unique mDNS hostname.

**Architecture:** A new build script produces one merged binary per board into `web/public/firmware/`, plus a `firmware.json` manifest. The web UI fetches the manifest, shows a board picker, conditionally shows WiFi fields, validates the chip detected by esptool-js against the chosen board, and flashes `merged-image @ 0x0` + `nvs @ 0x9000`. The Rust agent reads `device/hostname` from NVS and falls back to `zenclaw-XXYYZZ` (lower 3 bytes of MAC) so two CLI-flashed devices on the same network do not collide.

**Tech Stack:** Rust + esp-idf-svc 0.52 (agent firmware), Nuxt 4 + Vue 3 + esptool-js 0.6 (web UI), espflash CLI (build artifacts).

**Spec:** `docs/superpowers/specs/2026-04-29-rust-provisioning-pivot-design.md`

---

## File Map

| File | Status | Responsibility |
|------|--------|----------------|
| `agent-esp32/src/main.rs` | Modify | Add `resolve_hostname()`; wire mDNS to it; update log line |
| `scripts/build-rust-firmware.sh` | Create | Builds all 3 boards, emits merged `.bin` per board + `firmware.json` |
| `web/public/firmware/zenclaw-devkitc.bin` | Create | Merged S3+PSRAM image |
| `web/public/firmware/zenclaw-sdcard.bin` | Create | Merged S3 no-PSRAM image |
| `web/public/firmware/zenclaw-guition-p4.bin` | Create | Merged P4 image |
| `web/public/firmware/firmware.json` | Create | Board manifest |
| `web/public/firmware/README.md` | Modify | Point to build script |
| `web/public/firmware/micropython.bin` | Delete | Replaced by Rust |
| `web/public/firmware/zenclaw.img` | Delete | Replaced by Rust |
| `web/app/types/firmware.ts` | Create | `BoardManifest` interface, fallback boards constant |
| `web/app/composables/useSerial.ts` | Modify | New `flashDevice` signature + chip check + conditional NVS + new offsets |
| `web/app/pages/provision.vue` | Modify | Board picker, conditional WiFi fields, configValid, flash() call |

---

## Task 1: Rust agent — NVS-driven mDNS hostname with MAC fallback

**Goal:** `agent-esp32/src/main.rs` reads hostname from NVS namespace `device`, key `hostname`. If absent, falls back to `zenclaw-XXYYZZ` derived from the lower 3 bytes of the WiFi-STA MAC. Wire to `mdns.set_hostname(...)` and the HTTP server log line.

**Files:**
- Modify: `agent-esp32/src/main.rs:128-136` (mDNS block) and `:1217` (HTTP log line); add helper functions

**Why this task is first:** The web UI's NVS-write flow already exists (`web/app/composables/useSerial.ts:208`). Until the Rust agent reads it, every device announces `zenclaw.local` and collides. Doing this first lets the rest of the plan be tested end-to-end.

- [ ] **Step 1: Add a host-side unit test for the MAC suffix helper**

Open `agent-esp32/src/main.rs`. Find the bottom of the file. Add at the very end of the file (outside any `#[cfg]` block):

```rust
#[cfg(test)]
mod hostname_tests {
    fn format_mac_suffix(mac: &[u8; 6]) -> String {
        format!("zenclaw-{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5])
    }

    #[test]
    fn format_mac_suffix_uses_lower_three_bytes_lowercase_hex() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        assert_eq!(format_mac_suffix(&mac), "zenclaw-ddeeff");
    }

    #[test]
    fn format_mac_suffix_zero_pads_each_byte() {
        let mac = [0x00, 0x00, 0x00, 0x01, 0x02, 0x03];
        assert_eq!(format_mac_suffix(&mac), "zenclaw-010203");
    }
}
```

(The helper is duplicated inside the test module on purpose — main.rs has feature-gated functions which makes a single shared host-buildable helper awkward. The real implementation in step 3 is byte-identical.)

- [ ] **Step 2: Run the test to verify it passes**

```bash
cd agent-esp32 && cargo test --target $(rustc -vV | sed -n 's/host: //p') --no-default-features hostname_tests
```

Expected: 2 passed, 0 failed.

(We pass `--no-default-features` to avoid pulling esp-idf-svc into the host build.)

- [ ] **Step 3: Add `resolve_hostname()` and `read_device_hostname()` helpers to `main.rs`**

In `agent-esp32/src/main.rs`, **after the existing `nvs_get_string` function** (currently at `:227-246`), insert:

```rust
#[cfg(feature = "esp32")]
fn read_device_hostname(nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition) -> Option<String> {
    let handle = esp_idf_svc::nvs::EspNvs::new(nvs.clone(), "device", false).ok()?;
    nvs_get_string(&handle, "hostname").filter(|s| !s.is_empty())
}

#[cfg(feature = "esp32")]
fn resolve_hostname(nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition) -> String {
    if let Some(h) = read_device_hostname(nvs) {
        return h;
    }
    let mut mac = [0u8; 6];
    let err = unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA,
        )
    };
    if err != 0 {
        log::warn!("esp_read_mac failed: {} — using static fallback", err);
        return "zenclaw".to_string();
    }
    format!("zenclaw-{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5])
}
```

Note: `esp_mac_type_t_ESP_MAC_WIFI_STA` works on both ESP32-S3 (Xtensa) and ESP32-P4 (RISC-V); the P4 has no internal WiFi but the efuse MAC block still exposes the WIFI_STA address as the chip's base MAC. Confirmed by checking `target/.../bindings.rs` — both targets export the symbol.

- [ ] **Step 4: Wire mDNS to `resolve_hostname()`**

In `agent-esp32/src/main.rs`, replace the mDNS block at `:128-136`:

```rust
    // --- mDNS ---
    #[cfg(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled))]
    let hostname = {
        let h = resolve_hostname(&nvs);
        let mut mdns = esp_idf_svc::mdns::EspMdns::take().unwrap();
        mdns.set_hostname(&h).unwrap();
        mdns.set_instance_name("ZenClaw Agent").unwrap();
        mdns.add_service(None, "_http", "_tcp", 80, &[]).unwrap();
        log::info!("mDNS: {}.local", h);
        std::mem::forget(mdns);
        h
    };
    #[cfg(not(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled)))]
    let hostname = {
        log::warn!("mDNS: not available (needs cargo clean && cargo build)");
        resolve_hostname(&nvs)
    };
```

This binds `hostname` for use later in the file. The block returns the resolved string regardless of mDNS feature gating.

- [ ] **Step 5: Update HTTP server log line**

In `agent-esp32/src/main.rs`, find the line at `:1217` (originally `log::info!("HTTP server on :80 — http://{}/ or http://zenclaw.local/", ip_str);`). Replace the literal `zenclaw.local` with the resolved hostname:

```rust
    log::info!("HTTP server on :80 — http://{}/ or http://{}.local/", ip_str, hostname);
```

- [ ] **Step 6: Build for one Rust target to confirm it compiles**

```bash
cd agent-esp32 && just build devkitc
```

Expected: clean compile, no errors. (Warnings about unused imports are fine.)

If the linker complains that `hostname` is moved or borrowed twice, the most likely cause is a duplicate `let hostname = ...` introduced by accident — re-read step 4, there should be exactly one `let hostname` at module scope and it must be used immutably afterward.

- [ ] **Step 7: Commit**

```bash
git add agent-esp32/src/main.rs
git commit -m "$(cat <<'EOF'
feat(esp32): NVS-driven mDNS hostname with MAC fallback

Reads device/hostname from NVS so multiple devices on the same network
can coexist with unique names. Falls back to zenclaw-XXYYZZ derived from
the lower 3 bytes of the WiFi-STA MAC for CLI-flashed devices that have
not been provisioned via the web UI.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Build script for merged Rust firmware images

**Goal:** A single shell script that builds all three boards via `just build` and converts each ELF into a chip-correct merged binary at `web/public/firmware/zenclaw-<board>.bin`, then writes `firmware.json`.

**Files:**
- Create: `scripts/build-rust-firmware.sh` (executable)

- [ ] **Step 1: Create the build script**

Write `scripts/build-rust-firmware.sh`:

```bash
#!/usr/bin/env bash
# Build merged Rust firmware images for the web UI provisioning wizard.
#
# Output: web/public/firmware/zenclaw-<board>.bin (one per board)
#         web/public/firmware/firmware.json (board manifest)
#
# Requires: just, espflash 3.x, the Xtensa Rust toolchain pinned to 1.93.0.0
# (see CLAUDE.md → S3 Xtensa LLVM bug pitfall).
#
# Usage: ./scripts/build-rust-firmware.sh [board1 board2 ...]
#        Defaults to all three boards if none specified.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
AGENT_DIR="$REPO_ROOT/agent-esp32"
OUTPUT_DIR="$REPO_ROOT/web/public/firmware"

cd "$AGENT_DIR"

declare -a BOARDS
if [[ $# -gt 0 ]]; then
    BOARDS=("$@")
else
    BOARDS=(devkitc sdcard guition-p4)
fi

mkdir -p "$OUTPUT_DIR"

# Manifest entries written as we go.
declare -a MANIFEST_ENTRIES

read_manifest_field() {
    local board="$1" field="$2"
    awk -F' *= *' -v f="^$field\$" '$1 ~ f {gsub(/"/,"",$2); print $2; exit}' "boards/$board.toml"
}

# Per-board human-friendly metadata that lives only in this script.
# Keep this in sync with boards/<id>.toml descriptions.
board_display_name() {
    case "$1" in
        devkitc)    echo "ESP32-S3 DevKitC" ;;
        sdcard)     echo "LILYGO T-Dongle-S3" ;;
        guition-p4) echo "Guition JC-ESP32P4-M3-DEV" ;;
        *)          echo "$1" ;;
    esac
}

board_chip_label() {
    case "$1" in
        devkitc|sdcard) echo "ESP32-S3" ;;
        guition-p4)     echo "ESP32-P4" ;;
        *)              echo "ESP32" ;;
    esac
}

board_network() {
    case "$1" in
        guition-p4) echo "ethernet" ;;
        *)          echo "wifi" ;;
    esac
}

board_default() {
    [[ "$1" == "devkitc" ]] && echo true || echo false
}

board_description() {
    case "$1" in
        devkitc)    echo "8MB PSRAM, USB Host capable" ;;
        sdcard)     echo "No PSRAM, SD card slot" ;;
        guition-p4) echo "32MB PSRAM, Ethernet via IP101 PHY" ;;
        *)          echo "" ;;
    esac
}

for board in "${BOARDS[@]}"; do
    [[ -f "boards/$board.toml" ]] || { echo "unknown board: $board" >&2; exit 1; }

    chip=$(read_manifest_field "$board" chip)
    target=$(read_manifest_field "$board" target)
    bootloader=$(read_manifest_field "$board" bootloader)

    elf="target/$target/release/zenclaw-agent"
    out="$OUTPUT_DIR/zenclaw-$board.bin"

    echo "==> Building $board ($chip / $target)"
    just build "$board" --release

    echo "==> Saving merged image -> $out"
    espflash save-image \
        --chip "$chip" \
        --partition-table partitions.csv \
        --bootloader "$bootloader" \
        --merge \
        --skip-padding \
        "$elf" "$out"

    size=$(stat -c %s "$out")
    echo "    $(basename "$out"): ${size} bytes"

    MANIFEST_ENTRIES+=("$(cat <<JSON
    {
      "id": "$board",
      "name": "$(board_display_name "$board")",
      "chip": "$(board_chip_label "$board")",
      "image": "zenclaw-$board.bin",
      "network": "$(board_network "$board")",
      "default": $(board_default "$board"),
      "description": "$(board_description "$board")"
    }
JSON
)")
done

# Join manifest entries with commas
joined=""
for entry in "${MANIFEST_ENTRIES[@]}"; do
    if [[ -z "$joined" ]]; then
        joined="$entry"
    else
        joined="$joined,
$entry"
    fi
done

cat > "$OUTPUT_DIR/firmware.json" <<JSON
{
  "boards": [
$joined
  ]
}
JSON

echo "==> Wrote $OUTPUT_DIR/firmware.json"
echo
echo "Done. Outputs:"
ls -lh "$OUTPUT_DIR"/zenclaw-*.bin "$OUTPUT_DIR/firmware.json"
```

- [ ] **Step 2: Mark it executable**

```bash
chmod +x scripts/build-rust-firmware.sh
```

- [ ] **Step 3: Update `web/public/firmware/README.md`**

Replace the existing two-line file with:

```markdown
Rust firmware artifacts for the web UI provisioning wizard.

Build with: ../../../scripts/build-rust-firmware.sh

Outputs:
- zenclaw-devkitc.bin     ESP32-S3 DevKitC (PSRAM)
- zenclaw-sdcard.bin      LILYGO T-Dongle-S3 (no PSRAM)
- zenclaw-guition-p4.bin  Guition JC-ESP32P4-M3-DEV (Ethernet)
- firmware.json           Board manifest consumed by provision.vue
```

- [ ] **Step 4: Commit**

```bash
git add scripts/build-rust-firmware.sh web/public/firmware/README.md
git commit -m "$(cat <<'EOF'
build: script to produce merged Rust firmware images for web UI

Builds all three Rust target boards and saves chip-correct merged
images plus a firmware.json manifest into web/public/firmware/.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Generate firmware artifacts and replace MicroPython files

**Goal:** Run the build script, replace the committed MicroPython artifacts with the Rust ones, and commit the binaries so the web UI ships with them.

**Files:**
- Create: `web/public/firmware/zenclaw-devkitc.bin`
- Create: `web/public/firmware/zenclaw-sdcard.bin`
- Create: `web/public/firmware/zenclaw-guition-p4.bin`
- Create: `web/public/firmware/firmware.json`
- Delete: `web/public/firmware/micropython.bin`
- Delete: `web/public/firmware/zenclaw.img`

- [ ] **Step 1: Run the build script**

```bash
./scripts/build-rust-firmware.sh
```

Expected: three `Building <board>` blocks, three `Saving merged image` lines, a `Wrote firmware.json` line. Total runtime depends on whether `target/` is warm — first run can take 5-10 min for ESP32-S3 builds, P4 takes ~3 min. Subsequent runs (only ELF re-link if Rust source unchanged) are <30s per board.

If a build fails for a board, fix the underlying issue before continuing. Common causes:
- Xtensa toolchain not pinned to 1.93.0.0 → see CLAUDE.md
- esp-idf-sys cache stale after switching boards → `just clean <board>` and retry

- [ ] **Step 2: Sanity-check the output**

```bash
ls -lh web/public/firmware/
```

Expected: three `zenclaw-*.bin` files between roughly 1 MB and 3 MB each, plus `firmware.json` (about 600 bytes).

```bash
cat web/public/firmware/firmware.json
```

Expected: Valid JSON with three board entries, DevKitC has `"default": true`, Guition P4 has `"network": "ethernet"`, others have `"network": "wifi"`.

- [ ] **Step 3: Delete the MicroPython artifacts**

```bash
git rm web/public/firmware/micropython.bin web/public/firmware/zenclaw.img
```

- [ ] **Step 4: Commit the artifacts**

```bash
git add web/public/firmware/zenclaw-*.bin web/public/firmware/firmware.json
git commit -m "$(cat <<'EOF'
build(web): replace MicroPython artifacts with Rust merged images

Drops micropython.bin + zenclaw.img. Adds zenclaw-<board>.bin for
DevKitC, T-Dongle-S3, and Guition P4 plus the firmware.json manifest
the provisioning wizard consumes.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Web — `BoardManifest` type and manifest fetch

**Goal:** A typed module that exports the manifest interface and a fallback constant matching `firmware.json`. Keeps both `useSerial.ts` and `provision.vue` consistent.

**Files:**
- Create: `web/app/types/firmware.ts`

- [ ] **Step 1: Create the types module**

Write `web/app/types/firmware.ts`:

```ts
export type BoardChip = 'ESP32-S3' | 'ESP32-P4'
export type BoardNetwork = 'wifi' | 'ethernet'

export interface BoardManifest {
  id: string
  name: string
  chip: BoardChip
  image: string
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
    network: 'wifi',
    default: true,
    description: '8MB PSRAM, USB Host capable',
  },
  {
    id: 'sdcard',
    name: 'LILYGO T-Dongle-S3',
    chip: 'ESP32-S3',
    image: 'zenclaw-sdcard.bin',
    network: 'wifi',
    description: 'No PSRAM, SD card slot',
  },
  {
    id: 'guition-p4',
    name: 'Guition JC-ESP32P4-M3-DEV',
    chip: 'ESP32-P4',
    image: 'zenclaw-guition-p4.bin',
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
    const resp = await fetch(baseURL + 'firmware/firmware.json')
    if (!resp.ok) return FALLBACK_BOARDS
    const data = (await resp.json()) as FirmwareManifest
    if (!Array.isArray(data?.boards) || data.boards.length === 0) return FALLBACK_BOARDS
    // Validate every entry has the required fields before trusting the manifest.
    const valid = data.boards.filter(b =>
      typeof b.id === 'string' && b.id.length > 0
      && typeof b.name === 'string'
      && (b.chip === 'ESP32-S3' || b.chip === 'ESP32-P4')
      && typeof b.image === 'string'
      && (b.network === 'wifi' || b.network === 'ethernet'),
    )
    return valid.length > 0 ? valid : FALLBACK_BOARDS
  } catch {
    return FALLBACK_BOARDS
  }
}
```

- [ ] **Step 2: Verify it type-checks**

```bash
cd web && npx nuxt prepare && npx vue-tsc --noEmit
```

Expected: no errors. (`vue-tsc` may produce warnings about other files — only the new `types/firmware.ts` file matters here. If `vue-tsc` is not installed, fall back to `npx nuxt typecheck` if available, or skip and rely on the next task's compile pass.)

- [ ] **Step 3: Commit**

```bash
git add web/app/types/firmware.ts
git commit -m "$(cat <<'EOF'
feat(web): BoardManifest type + firmware.json loader

Loads board manifest from /firmware/firmware.json with a hardcoded
fallback if the file is missing or malformed.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Web — provisioning pivot (`useSerial.ts` + `provision.vue`)

**Goal:** Refactor `flashDevice` to accept a `BoardManifest`, validate chip-vs-board, build NVS conditionally, flash merged image at 0x0 + NVS at 0x9000. In the same commit, update `provision.vue` to add a board picker, hide WiFi fields for Ethernet boards, fetch the manifest at mount, and pass the selected board into `flashDevice`. Done as one task because the type changes in `useSerial.ts` immediately invalidate the existing call site in `provision.vue` — splitting would land a commit that doesn't typecheck.

**Files:**
- Modify: `web/app/composables/useSerial.ts` (interface + `flashDevice` body)
- Modify: `web/app/pages/provision.vue` (script + template)

- [ ] **Step 1: Update `useSerial.ts` imports and `DeviceConfig` interface**

In `web/app/composables/useSerial.ts`, replace the existing import line at the top:

```ts
import { buildNvsPartition } from '~/utils/nvs'
```

with:

```ts
import { buildNvsPartition, type NvsBlob } from '~/utils/nvs'
import type { BoardManifest } from '~/types/firmware'
```

Replace the existing `DeviceConfig` interface (currently lines 9-13):

```ts
export interface DeviceConfig {
  hostname: string
  board: BoardManifest
  ssid?: string      // required when board.network === 'wifi'
  password?: string  // required when board.network === 'wifi'
}
```

- [ ] **Step 2: Replace the firmware fetch + NVS build + writeFlash block in `flashDevice`**

In the same file, find the block starting with the comment `// Download firmware files` (around line 186) and ending with the closing `})` of `loader.writeFlash` (around line 229). Also remove the three preceding lines that read `const chipName = loader.chip?.CHIP_NAME || 'ESP32'`, `log(...)`, and `onProgress({ ... percent: 10, message: \`Chip: ${chipName}\` })` (around lines 182-184). Replace that whole region with:

```ts
      // --- Chip-vs-board guard ---
      const detectedChip = loader.chip?.CHIP_NAME || 'ESP32'
      log(`Chip detected: ${detectedChip}`)
      onProgress({ stage: 'connecting', percent: 10, message: `Chip: ${detectedChip}` })
      if (detectedChip !== config.board.chip) {
        throw new Error(
          `Selected ${config.board.name} (${config.board.chip}) but detected ${detectedChip}. `
          + `Plug in the correct board or change selection.`,
        )
      }

      // --- Download merged firmware image ---
      onProgress({ stage: 'flashing', percent: 15, message: 'Downloading firmware...' })
      const base = useRuntimeConfig().app.baseURL
      const fwResponse = await fetch(base + 'firmware/' + config.board.image)
      if (!fwResponse.ok) {
        throw new Error(
          `Firmware ${config.board.image} missing (HTTP ${fwResponse.status}) — `
          + `rebuild via scripts/build-rust-firmware.sh`,
        )
      }
      const fwData = new Uint8Array(await fwResponse.arrayBuffer())
      log(`Firmware: ${fwData.length} bytes`)

      // --- Build NVS partition ---
      const nvsEntries: NvsBlob[] = [
        { namespace: 'device', key: 'hostname', value: config.hostname },
      ]
      if (config.board.network === 'wifi') {
        if (!config.ssid) throw new Error('WiFi SSID is required for this board')
        nvsEntries.push(
          { namespace: 'wifi', key: 'ssid', value: config.ssid },
          { namespace: 'wifi', key: 'password', value: config.password ?? '' },
        )
        log(`Building NVS: hostname=${config.hostname}, WiFi=${config.ssid}`)
      } else {
        log(`Building NVS: hostname=${config.hostname} (Ethernet — no WiFi creds)`)
      }
      const nvsData = buildNvsPartition(nvsEntries)

      // --- Flash merged image + NVS ---
      log('Flashing firmware + NVS...')
      onProgress({ stage: 'flashing', percent: 25, message: 'Flashing...' })
      await loader.writeFlash({
        fileArray: [
          { data: fwData,  address: 0x0 },     // bootloader + partition table + app (chip-correct internal layout)
          { data: nvsData, address: 0x9000 },  // NVS partition (hostname + WiFi creds if applicable)
        ],
        flashSize: 'keep',
        flashMode: 'keep',
        flashFreq: 'keep',
        eraseAll: true,
        compress: true,
        reportProgress: (_fileIndex: number, written: number, total: number) => {
          const pct = 25 + Math.round((written / total) * 70)
          onProgress({ stage: 'flashing', percent: pct, message: `${written}/${total} bytes` })
        },
      })
```

- [ ] **Step 3: Confirm no MicroPython references remain in `useSerial.ts`**

```bash
grep -n "micropython\|zenclaw\.img" web/app/composables/useSerial.ts
```

Expected: no matches. If any remain, delete those lines.

- [ ] **Step 4: Add manifest state to `provision.vue` script**

In `web/app/pages/provision.vue`, add at the top of `<script setup>` (after the existing imports at lines 1-3):

```ts
import { loadBoardManifest, FALLBACK_BOARDS, type BoardManifest } from '~/types/firmware'
```

After the existing reactive refs (around line 25, just after `const deviceName = ref(randomName())`), add:

```ts
const boards = ref<BoardManifest[]>(FALLBACK_BOARDS)
const boardId = ref<string>(FALLBACK_BOARDS.find(b => b.default)?.id ?? FALLBACK_BOARDS[0]!.id)
const selectedBoard = computed<BoardManifest>(() =>
  boards.value.find(b => b.id === boardId.value) ?? boards.value[0]!,
)
const boardItems = computed(() =>
  boards.value.map(b => ({ label: `${b.name} (${b.chip})`, value: b.id })),
)
```

- [ ] **Step 5: Fetch the manifest in `onMounted`**

In the existing `onMounted(async () => { ... })` block (around line 107), add as the very first line of the async body, before `await fetchModels()`:

```ts
  boards.value = await loadBoardManifest(useRuntimeConfig().app.baseURL)
  if (!boards.value.some(b => b.id === boardId.value)) {
    boardId.value = boards.value.find(b => b.default)?.id ?? boards.value[0]!.id
  }
```

- [ ] **Step 6: Persist and restore `boardId` in localStorage**

In the watcher that saves to localStorage (around line 149-159), update the watch list and JSON body to include `boardId`:

```ts
watch([wifiSsid, wifiPassword, apiKey, apiProvider, apiModel, baseUrl, deviceName, boardId], () => {
  localStorage.setItem(STORAGE_KEY, JSON.stringify({
    wifiSsid: wifiSsid.value,
    wifiPassword: wifiPassword.value,
    apiKey: apiKey.value,
    apiProvider: apiProvider.value,
    apiModel: apiModel.value,
    baseUrl: baseUrl.value,
    deviceName: deviceName.value,
    boardId: boardId.value,
  }))
})
```

In the restoring block inside `onMounted` (around lines 110-119), add this line anywhere inside the existing `if (saved)` body:

```ts
    if (data.boardId) boardId.value = data.boardId
```

- [ ] **Step 7: Update `configValid`**

Replace the existing `configValid` computed (around line 139):

```ts
const configValid = computed(() => {
  if (!apiKey.value) return false
  if (selectedBoard.value.network === 'wifi' && !wifiSsid.value) return false
  return true
})
```

- [ ] **Step 8: Update `flash()` to pass the selected board**

Replace the body of the `flash()` function (around line 175):

```ts
async function flash() {
  flashing.value = true
  error.value = null

  const ok = await serial.flashDevice(
    {
      hostname: deviceName.value,
      board: selectedBoard.value,
      ssid: selectedBoard.value.network === 'wifi' ? wifiSsid.value : undefined,
      password: selectedBoard.value.network === 'wifi' ? wifiPassword.value : undefined,
    },
    (p) => { progress.value = p },
  )

  if (ok) {
    active.value = 2
    pollForDevice()
  } else {
    error.value = progress.value.message
  }
  flashing.value = false
}
```

- [ ] **Step 9: Add the board picker to the Configure step template**

In the template, find the Configure step block (starts at `<div v-if="item.title === 'Configure'"`, around line 248). Insert this as the very first children of that block (before the existing WiFi `<UFormField label="WiFi SSID">`):

```vue
          <UFormField label="Board" class="w-full">
            <USelectMenu
              v-model="boardId"
              class="w-full"
              size="xl"
              :items="boardItems"
              value-key="value"
            />
          </UFormField>
          <p v-if="selectedBoard.description" class="text-xs text-dimmed">
            {{ selectedBoard.description }}
          </p>

          <USeparator />
```

- [ ] **Step 10: Wrap WiFi fields and add Ethernet alternative**

In the same Configure block, replace the two existing WiFi `UFormField` blocks (`WiFi SSID` and `WiFi Password`, around lines 249-254) with:

```vue
          <template v-if="selectedBoard.network === 'wifi'">
            <UFormField label="WiFi SSID" class="w-full">
              <UInput v-model="wifiSsid" placeholder="Your WiFi network" class="w-full" size="xl" />
            </UFormField>
            <UFormField label="WiFi Password" class="w-full">
              <UInput v-model="wifiPassword" class="w-full" size="xl" />
            </UFormField>
          </template>
          <div v-else class="rounded border border-default bg-elevated p-3 text-sm text-muted">
            <p class="font-semibold text-toned mb-1">Ethernet device</p>
            <p>Plug an Ethernet cable into the device before flashing — no WiFi credentials needed.</p>
          </div>
```

- [ ] **Step 11: Update Flash step copy**

In the Flash step block (`<div v-else-if="item.title === 'Flash'"`, around line 306), replace the introductory `<p>` (the one that says "Plug your ESP32-S3 into this computer via USB...") with:

```vue
          <p class="text-sm text-muted">
            Plug your <strong>{{ selectedBoard.name }}</strong> ({{ selectedBoard.chip }}) into this
            computer via USB and click Flash.
          </p>
          <p v-if="selectedBoard.network === 'ethernet'" class="text-sm text-muted">
            Make sure the Ethernet cable is connected before the device boots — the agent gets its
            network address via DHCP.
          </p>
```

- [ ] **Step 12: Type-check**

```bash
cd web && npx nuxt prepare
```

Expected: no errors. If `nuxt prepare` reports type errors, the most likely cause is a mistake in the imports, the manifest type usage, or a missing `selectedBoard.value` access — re-read the steps and look at the exact line numbers reported.

- [ ] **Step 13: Smoke-test in dev**

```bash
cd web && npm run dev
```

In a browser at `http://localhost:3000/provision`:
- The Board dropdown shows three options
- "Guition JC-ESP32P4-M3-DEV" → WiFi fields disappear, Ethernet hint appears
- "ESP32-S3 DevKitC" → WiFi fields reappear
- "LILYGO T-Dongle-S3" → WiFi fields reappear with different description

Stop the dev server (Ctrl-C).

- [ ] **Step 14: Commit**

```bash
git add web/app/composables/useSerial.ts web/app/pages/provision.vue
git commit -m "$(cat <<'EOF'
feat(web): pivot provisioning wizard to Rust agent

flashDevice now takes a BoardManifest, validates the esptool-js chip
handshake against the chosen board, builds NVS conditionally (WiFi
creds only for wifi-network boards), and flashes the merged image at
0x0 plus NVS at 0x9000.

provision.vue adds a Board dropdown driven by firmware.json, hides
WiFi fields for Ethernet-only boards (Guition P4), and passes the
selected board into flashDevice.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Manual end-to-end verification per board

**Goal:** Confirm the wizard flashes each of the three boards from a fresh state, that two devices on the same network do not collide, and that chip-mismatch is caught.

**Files:** None modified. This task is verification only.

**Pre-requisites:** All three physical boards available. WiFi network credentials. A Cloudflare R2 (optional — only needed if validating cloud parity, otherwise skip cloud step). USB-C cable. (For Guition P4: Ethernet cable plugged into a router with DHCP.)

- [ ] **Step 1: Verify DevKitC**

Start the dev server: `cd web && npm run dev`. Open `http://localhost:3000/provision`.

Configure step:
- Board: ESP32-S3 DevKitC (default)
- WiFi: real credentials
- API key: any valid Gemini key
- Device name: leave the auto-generated one (e.g. `zenclaw-swift-fox`)
- Click Next

Flash step:
- Plug DevKitC via USB-C
- Hold BOOT, press RESET, release BOOT
- Click Flash, select "USB JTAG/serial debug unit"
- Wait for flash to complete (1-2 min)
- Watchdog reset triggers reboot

Connect step:
- Wait for `<device-name>.local` to come online (~12s after flash)
- API config gets pushed automatically
- Stepper advances to Done

Verify on device: `curl -sf http://<device-name>.local/api/status | python3 -m json.tool` — confirm `agent_name`, `network.kind=wifi`, `chip=esp32s3`.

- [ ] **Step 2: Verify Guition P4**

In the wizard:
- Reset to step 0, board = Guition JC-ESP32P4-M3-DEV
- WiFi fields should disappear, Ethernet hint visible
- Set a different device name (e.g. add `-p4` suffix to keep the previous one too)
- Plug Ethernet cable first, then USB-C

Flash, wait for reboot. P4 should announce its hostname via mDNS over Ethernet within ~5s.

`curl -sf http://<p4-name>.local/api/status` — confirm `network.kind=ethernet`, `chip=esp32p4`.

- [ ] **Step 3: Verify both devices coexist on the same network**

With both DevKitC and P4 flashed and online:

```bash
curl -sf http://<devkitc-name>.local/api/status | python3 -c "import sys,json; d=json.load(sys.stdin); print('host', d.get('agent_name'), 'ip', d.get('network',{}).get('ip'))"
curl -sf http://<p4-name>.local/api/status | python3 -c "import sys,json; d=json.load(sys.stdin); print('host', d.get('agent_name'), 'ip', d.get('network',{}).get('ip'))"
```

Both should resolve to different IPs and respond. If one shadowing the other, the NVS hostname write or the Rust `resolve_hostname` is broken — re-check Task 1 step 4.

- [ ] **Step 4: Verify chip-mismatch guard**

In the wizard, set Board = Guition P4. Plug in the DevKitC. Click Flash.

Expected: error banner reads `Selected Guition JC-ESP32P4-M3-DEV (ESP32-P4) but detected ESP32-S3. Plug in the correct board or change selection.` No flash actually happens; the DevKitC is unmodified.

Repeat with Board = DevKitC, plug Guition P4 — symmetrical error.

- [ ] **Step 5: Verify T-Dongle-S3 (if hardware available)**

If you have a T-Dongle-S3 wired up: same as DevKitC but pick `LILYGO T-Dongle-S3` board. Confirm the device boots without a PSRAM crash (the no-PSRAM image was used).

If T-Dongle-S3 hardware is not at hand, mark this step as skipped in the commit message and note the gap — manual verification of the no-PSRAM image is still pending.

- [ ] **Step 6: Push the branch**

```bash
git push origin feat/rust-agent
```

- [ ] **Step 7: Update memory with parity completion**

Append to `/home/ben/.claude/projects/-home-ben-repos-zenclaw/memory/MEMORY.md` an entry like:

```
- [Web provisioning pivoted to Rust](project_provisioning_pivot.md) — Web UI flashes Rust agent (3 boards, chip-mismatch guard, NVS hostname). MicroPython artifacts removed.
```

And write the corresponding memory file at `/home/ben/.claude/projects/-home-ben-repos-zenclaw/memory/project_provisioning_pivot.md` summarizing what shipped, what was deferred (T-Dongle if skipped), and the git SHA at the head of the branch.

---

## Self-review notes

- Spec coverage:
  - Build artifacts → Task 2, 3
  - Per-board merged image with chip-correct internal layout → Task 2 (`espflash save-image --merge`)
  - `firmware.json` schema and fallback → Task 2 (script writes), Task 4 (TS loader + fallback)
  - Board picker → Task 5 step 9
  - Conditional WiFi fields → Task 5 step 10
  - Chip mismatch guard → Task 5 step 2
  - NVS conditional construction → Task 5 step 2
  - Rust NVS hostname read + MAC fallback → Task 1
  - MicroPython artifact deletion → Task 3 step 3
  - Manual verification across boards → Task 6
- All steps include the actual code or commands. No "TBD" or "implement appropriately" placeholders.
- Type names consistent: `BoardManifest` used everywhere. `network: 'wifi' | 'ethernet'`. `chip: 'ESP32-S3' | 'ESP32-P4'`. `flashDevice(config: DeviceConfig, ...)` where `config.board: BoardManifest`.
- Commit boundaries: 6 commits, each builds-and-passes-tests in isolation. Web changes are bundled into one Task 5 commit because `useSerial.ts`'s new required `board: BoardManifest` field would break the call site in `provision.vue` if landed separately.
