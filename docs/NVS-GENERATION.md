# NVS Partition Binary Generation

## Problem

The web flasher needs to generate an NVS partition binary in the browser to bake WiFi credentials and device hostname directly into the ESP32's flash. This avoids needing serial config after flashing.

## What Works

ESP-IDF's official `nvs_partition_gen.py` (v4.4.8) generates correct binaries:

```bash
# Install (no pip package — grab from ESP-IDF repo)
curl -sL "https://raw.githubusercontent.com/espressif/esp-idf/v4.4.8/components/nvs_flash/nvs_partition_generator/nvs_partition_gen.py" -o nvs_partition_gen.py

# CSV format for blob entries (read by MicroPython's NVS.get_blob())
cat > provision.csv << 'EOF'
key,type,encoding,value
wifi,namespace,,
ssid,file,binary,/tmp/ssid.bin
password,file,binary,/tmp/password.bin
device,namespace,,
hostname,file,binary,/tmp/hostname.bin
EOF

# Generate (0x6000 = 24KB, standard NVS partition size)
python3 nvs_partition_gen.py generate provision.csv nvs.bin 0x6000

# Flash (must erase region first to clear runtime NVS pages)
esptool.py erase_region 0x9000 0x6000
esptool.py write_flash 0x9000 nvs.bin
```

**Critical**: Use `file,binary` encoding, NOT `data,string`. MicroPython's `NVS.get_blob()` reads type 0x42 (BLOB_DATA) entries. `data,string` produces type 0x21 (STRING) which `get_blob()` cannot read.

## TypeScript Generator (Fixed)

The TypeScript NVS generator (`web/app/utils/nvs.ts`) had two CRC32 bugs, now fixed:

1. **CRC32 initial value**: ESP-IDF's `esp_rom_crc32_le(0xFFFFFFFF, ...)` pre-XORs the seed internally (`~0xFFFFFFFF = 0x00000000`). Our code was starting the accumulator at `0xFFFFFFFF` directly (standard CRC32). Fix: start at `0x00000000`.

2. **Data CRC scope**: ESP-IDF CRCs only the raw data bytes (`dataSize`), not the 0xFF-padded entry-aligned buffer. Fix: `crc32(data)` instead of `crc32(dataArea)`.

## Additional Gotchas

1. **System overwrites page 0**: On boot, ESP-IDF's WiFi/PHY init writes ~70 entries (misc, nvs.net80211, phy, cal_data) to NVS page 0. If you flash NVS without erasing the region first, the system's runtime pages (with higher sequence numbers) take precedence over your freshly-flashed page 0.

2. **Must erase full NVS region**: `esptool write_flash` erases the sectors it writes to, but NVS uses 6 pages across 24KB. The system may have written to pages beyond page 0. Always `erase_region 0x9000 0x6000` before writing.

3. **eraseAll in esptool-js**: Using `eraseAll: true` in `writeFlash()` erases the entire flash first, which solves the stale NVS page problem when doing a full provision (firmware + NVS + filesystem together).

## Flash Layout (ESP32-S3, 16MB flash, official MicroPython ESP32_GENERIC_S3-SPIRAM_OCT v1.27.0)

| Partition | Offset | Size | Notes |
|-----------|--------|------|-------|
| bootloader | 0x0 | ~28KB | Part of micropython.bin |
| partition table | 0x8000 | 4KB | Part of micropython.bin |
| nvs | 0x9000 | 24KB (0x6000) | WiFi creds, device config |
| phy_init | 0xF000 | 4KB | RF calibration (auto-generated) |
| factory | 0x10000 | ~1.9MB (0x1F0000) | MicroPython app |
| vfs | 0x200000 | 14MB (0xE00000) | littlefs filesystem (auto-sized) |

## Status

TypeScript CRC32 bugs fixed — NVS binary generation now works in-browser. The `_provision.json` fallback in boot.py remains as a secondary path.
