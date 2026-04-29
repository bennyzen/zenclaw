# Rust Provisioning Pivot — Design

**Status:** Ready for implementation
**Date:** 2026-04-29
**Branch:** `feat/rust-agent`

## Goal

Replace the MicroPython browser provisioning flow with a Rust-agent equivalent that supports all three Rust target boards (ESP32-S3 DevKitC, LILYGO T-Dongle-S3, Guition ESP32-P4), does not require WiFi credentials for Ethernet-only boards, and lets multiple devices coexist on the same network via per-device hostnames.

## Background

The web UI at `web/app/pages/provision.vue` currently flashes the MicroPython firmware (`web/public/firmware/micropython.bin` + `zenclaw.img`) over Web Serial using `useSerial.ts:flashDevice()`. The Rust agent at `agent-esp32/` has reached full feature parity (chat, memory, SOUL, R2 cloud storage all verified live on Guition P4 — see `project_r2_cloud_status.md`), so the wizard should now flash the Rust agent instead.

Three constraints shape the design:
1. The Rust app binary is per-board, not per-chip — `sdkconfig.board.devkitc` enables PSRAM while `sdkconfig.board.sdcard` does not, and flashing a PSRAM-enabled image onto T-Dongle-S3 crashes at boot. There is no universal S3 build.
2. ESP32-P4 boots via Ethernet (IP101 PHY) with no WiFi support; asking the user for WiFi credentials would be confusing and useless.
3. The Rust agent currently hardcodes mDNS hostname to `zenclaw` (`agent-esp32/src/main.rs:131`), so two devices on one network collide. The MicroPython agent already reads `device/hostname` from NVS; the Rust agent needs the same.

## Architecture

Five components change:

1. **New build script** — `scripts/build-rust-firmware.sh` produces one merged `.bin` per board and a manifest.
2. **`web/public/firmware/`** — drop MicroPython artifacts; ship 3 board-specific images plus `firmware.json`.
3. **`web/app/composables/useSerial.ts`** — flash merged image at offset `0x0` plus NVS at `0x9000`; take a `board` parameter; verify chip handshake matches the selected board before writing.
4. **`web/app/pages/provision.vue`** — board-selector dropdown driven by `firmware.json`; conditional WiFi fields; chip-mismatch error path.
5. **`agent-esp32/src/main.rs`** — read mDNS hostname from NVS `device/hostname`, fall back to MAC-derived `zenclaw-XXYYZZ` for CLI-flashed devices.

Data flow remains: user fills form → Web Serial flash → device reboots → web UI polls `<hostname>.local` → `POST /api/config` pushes provider/key/model.

## Components

### Build artifacts

`scripts/build-rust-firmware.sh` — for each of `devkitc`, `sdcard`, `guition-p4`:

```bash
just build "$board" --release
chip=$(awk -F' *= *' '/^chip/ {gsub(/"/,"",$2); print $2}' "boards/$board.toml")
target=$(awk -F' *= *' '/^target/ {gsub(/"/,"",$2); print $2}' "boards/$board.toml")
bootloader=$(awk -F' *= *' '/^bootloader/ {gsub(/"/,"",$2); print $2}' "boards/$board.toml")

espflash save-image \
    --chip "$chip" \
    --partition-table partitions.csv \
    --bootloader "$bootloader" \
    --merge \
    "target/$target/release/zenclaw-agent" \
    "../web/public/firmware/zenclaw-$board.bin"
```

The `--merge` flag emits a single image with chip-correct internal layout: ESP32-S3 starts the bootloader at offset `0x0`, ESP32-P4 at `0x2000` (with leading `0xFF` padding), partition table at `0x8000`, app at `0x10000`. The image is therefore flashable at offset `0x0` regardless of chip.

After all three builds succeed, the script writes `firmware.json`:

```json
{
  "boards": [
    {
      "id": "devkitc",
      "name": "ESP32-S3 DevKitC",
      "chip": "ESP32-S3",
      "image": "zenclaw-devkitc.bin",
      "network": "wifi",
      "default": true,
      "description": "8MB PSRAM, USB Host capable"
    },
    {
      "id": "sdcard",
      "name": "LILYGO T-Dongle-S3",
      "chip": "ESP32-S3",
      "image": "zenclaw-sdcard.bin",
      "network": "wifi",
      "description": "No PSRAM, SD card slot"
    },
    {
      "id": "guition-p4",
      "name": "Guition JC-ESP32P4-M3-DEV",
      "chip": "ESP32-P4",
      "image": "zenclaw-guition-p4.bin",
      "network": "ethernet",
      "description": "32MB PSRAM, Ethernet via IP101 PHY"
    }
  ]
}
```

The script overwrites `web/public/firmware/README.md` with a one-liner pointing back to itself.

### `useSerial.ts:flashDevice` signature

```ts
export interface DeviceConfig {
  ssid?: string       // optional — WiFi boards only
  password?: string   // optional — WiFi boards only
  hostname: string
  board: BoardManifest // from firmware.json
}

interface BoardManifest {
  id: string
  name: string
  chip: 'ESP32-S3' | 'ESP32-P4'
  image: string
  network: 'wifi' | 'ethernet'
}
```

Flash sequence:
1. Web Serial port handshake (existing logic, unchanged)
2. `await loader.main()` — esptool-js reports `loader.chip.CHIP_NAME`
3. **Chip-vs-board guard:** if `loader.chip.CHIP_NAME !== board.chip`, throw `Selected ${board.name} (${board.chip}) but detected ${loader.chip.CHIP_NAME}. Plug in the correct board or change selection.`
4. `fetch(base + 'firmware/' + board.image)` → `Uint8Array`
5. Build NVS:
   ```ts
   const nvsEntries: NvsBlob[] = [
     { namespace: 'device', key: 'hostname', value: config.hostname },
   ]
   if (board.network === 'wifi') {
     nvsEntries.push(
       { namespace: 'wifi', key: 'ssid',     value: config.ssid! },
       { namespace: 'wifi', key: 'password', value: config.password! },
     )
   }
   const nvsData = buildNvsPartition(nvsEntries)
   ```
6. `loader.writeFlash({ fileArray: [{ data: image, address: 0x0 }, { data: nvsData, address: 0x9000 }], eraseAll: true, ... })`
7. Watchdog reset (existing logic, unchanged)

### `provision.vue` — Configure step

Add a board dropdown above the WiFi section, populated from `firmware.json`:

```vue
<UFormField label="Board" class="w-full">
  <USelectMenu
    v-model="boardId"
    :items="boardItems"
    value-key="value"
    size="xl"
    class="w-full"
  />
</UFormField>
<p class="text-xs text-dimmed">{{ selectedBoard?.description }}</p>
```

`boardItems` derives from `firmware.json` (one fetch on `onMounted`). Default selection: the entry with `default: true` (DevKitC).

WiFi fields wrapped in `v-if="selectedBoard?.network === 'wifi'"`. For Ethernet boards the WiFi block is replaced with:

```vue
<div v-else class="rounded border border-default bg-elevated p-3 text-sm text-muted">
  <p class="font-semibold text-toned mb-1">Ethernet device</p>
  <p>Plug an Ethernet cable into the device before flashing — no WiFi credentials needed.</p>
</div>
```

`configValid` becomes:
```ts
const configValid = computed(() => {
  if (!apiKey.value) return false
  if (selectedBoard.value?.network === 'wifi' && !wifiSsid.value) return false
  return true
})
```

`flash()` passes `board: selectedBoard.value` into `serial.flashDevice(...)`.

The error path for chip mismatch surfaces via the existing `error.value` ref — no new UI element needed.

### Rust agent — NVS hostname

`agent-esp32/src/main.rs:128-134` becomes:

```rust
#[cfg(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled))]
{
    let mut mdns = esp_idf_svc::mdns::EspMdns::take().unwrap();
    let hostname = resolve_hostname();
    mdns.set_hostname(&hostname).unwrap();
    mdns.set_instance_name("ZenClaw Agent").unwrap();
    mdns.add_service(None, "_http", "_tcp", 80, &[]).unwrap();
    log::info!("mDNS: {}.local", hostname);
    std::mem::forget(mdns);
}
```

`resolve_hostname()` (added in the same file or a small helper module):

```rust
fn resolve_hostname() -> String {
    if let Some(h) = read_nvs_string("device", "hostname") {
        if !h.is_empty() { return h; }
    }
    // Fallback: lower 3 bytes of MAC
    let mut mac = [0u8; 6];
    unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()); }
    format!("zenclaw-{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5])
}
```

`net/wifi_ui.rs` already has an NVS string reader for the `wifi` namespace. Generalize it to a public helper `pub fn read_nvs_string(namespace: &str, key: &str) -> Option<String>` and update the WiFi reader to call through. The change is mechanical (one signature change, two call sites). The MAC fallback uses `esp_idf_svc::sys::esp_read_mac(mac.as_mut_ptr(), esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA)` — verified available on both Xtensa and RISC-V toolchains.

The HTTP server log line at `main.rs:1217` is updated to use the resolved hostname instead of the literal `zenclaw.local`.

## Data flow

```
Configure step (provision.vue)
  ├── fetch firmware/firmware.json → boardItems[]
  ├── user picks board → selectedBoard reactive
  ├── WiFi fields visible iff network === 'wifi'
  └── randomName() suggests hostname (existing)

Flash step (useSerial.ts)
  ├── port handshake → loader.main()
  ├── loader.chip.CHIP_NAME
  │   ├── matches board.chip → continue
  │   └── mismatch → throw, surface via progress.error
  ├── fetch firmware/zenclaw-<board>.bin → Uint8Array
  ├── buildNvsPartition([device/hostname, ?wifi/ssid, ?wifi/password])
  ├── writeFlash([{image, 0x0}, {nvs, 0x9000}], eraseAll: true)
  └── watchdog reset

Connect step (provision.vue + useConnection)
  ├── poll http://<hostname>.local/api/status
  ├── on success: GET /api/config, merge providers, POST /api/config
  └── advance to Done
```

## Error handling

| Failure | Surface | Recovery |
|---------|---------|----------|
| Selected board's image is 404 | `progress.error = "Firmware <image> missing — rebuild via scripts/build-rust-firmware.sh"` | Build script run by maintainer; user retries |
| Chip detected ≠ board.chip | `progress.error = "Selected <board.name> (<board.chip>) but detected <CHIP_NAME>. Plug in correct board or change selection."` | User changes board or USB device, retries |
| `firmware.json` fetch fails (offline build, asset missing) | Fall back to a hardcoded copy compiled into provision.vue (3 entries, default DevKitC) | Page still functional |
| NVS build error | Existing try/catch; `progress.error` populated | User retries |
| `loader.writeFlash` failure | Existing try/catch; `progress.error` populated | User retries; existing serial monitor stays available for diagnosis |
| Device boots but `<hostname>.local` unreachable | Existing 30-attempt poll with 3s interval; "Retry" button | User checks WiFi credentials / Ethernet cable / router; retries |

## Out of scope (deferred)

- **MicroPython provisioning** — fully removed. `web/public/firmware/micropython.bin` and `zenclaw.img` are deleted. No fallback path. The MicroPython firmware itself stays in `firmware/` for desktop development per `CLAUDE.md`, but the web wizard no longer flashes it.
- **Vector embeddings** — already deferred per `CLAUDE.md` Deferred / TODO.
- **R2 cloud config in NVS** — the wizard does not provision R2 keys. Users configure those via the Config page after first boot, same as today.
- **OTA updates** — no support added; reflash is still done over USB.
- **Custom boards beyond the three above** — the design accommodates new entries in `firmware.json` without schema changes, but no UI for user-supplied images.

## Testing

**Unit (host-side, runs in `npm test` or equivalent):**
- `firmware.json` parse: missing fields, missing image, malformed JSON → graceful fallback to hardcoded list
- `buildNvsPartition` with WiFi entries omitted (Ethernet path) — verify partition is well-formed and `device/hostname` is the only blob

**Manual (per board, fresh device):**
- DevKitC: pick board, flash, device boots, mDNS announces `<hostname>.local`, `/api/config` POST succeeds
- T-Dongle-S3: pick board, flash, device boots without PSRAM crash, WiFi connects, mDNS announces
- Guition P4: pick board, no WiFi fields shown, flash, device boots, Ethernet DHCP completes, mDNS announces

**Manual (regression):**
- Pick DevKitC, plug in Guition P4 → chip-mismatch error, no flash performed
- Pick Guition P4, plug in DevKitC → chip-mismatch error
- Two DevKitCs on same network with different `randomName()` outputs → both reachable, no collision

**Manual (Rust agent fallback):**
- Flash a board via `just flash devkitc` (no NVS hostname write) → device announces `zenclaw-XXYYZZ.local` derived from MAC
- Provision the same board via web UI → device announces the user-chosen hostname instead

## Open questions

None — all earlier questions resolved during brainstorming. Key decisions documented above:
- Merged image per board (not separate bootloader/partition/app blobs)
- Board picker required (chip detection alone can't distinguish DevKitC from T-Dongle-S3)
- WiFi fields hidden for Ethernet boards
- MAC-derived hostname fallback for CLI flashes
