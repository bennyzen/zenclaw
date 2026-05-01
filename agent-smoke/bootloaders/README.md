# Vendored ESP-IDF Bootloaders

Pre-built bootloaders extracted from clean `esp-idf-sys` builds. We vendor them
because `espflash`'s default bundled bootloader has historically caused boot
loops on ESP32-S3 (and we treat P4 as similarly opaque until proven otherwise).

| File | Chip | ESP-IDF | Extracted | sha256 |
|---|---|---|---|---|
| esp32p4.bin | ESP32-P4 | v5.4 | 2026-04-29 | c63a1d725b5f3f3d5e956c7bec56da4d87beb73ac77e85d27e7c2f47df09347b |

## Re-extraction procedure

```bash
just clean guition-p4
just build guition-p4
cp target/riscv32imafc-esp-espidf/release/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin bootloaders/esp32p4.bin
sha256sum bootloaders/esp32p4.bin
# Update this README with the new sha256.
```
