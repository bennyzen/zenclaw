# Vendored ESP-IDF Bootloaders

Pre-built bootloaders extracted from clean `esp-idf-sys` builds. Vendored
because `espflash`'s default bundled bootloader has caused boot loops on
ESP32-S3.

These binaries originate from Espressif's [ESP-IDF](https://github.com/espressif/esp-idf)
(v5.4) and are redistributed under the **Apache License 2.0**. The ZenClaw MIT
license does not apply to these files; see the ESP-IDF
[LICENSE](https://github.com/espressif/esp-idf/blob/master/LICENSE) for terms.

| File | Chip | ESP-IDF | Extracted | sha256 |
|---|---|---|---|---|
| esp32s3.bin | ESP32-S3 | v5.4 | moved from `agent-esp32/bootloader.bin` 2026-04-29 | 39cf9ad29172c0751971562f5f82f0f0691182c75e8225e267a8f2ba68cfa6ff |
| esp32p4.bin | ESP32-P4 | v5.4 | 2026-04-29 (from `agent-esp32-smoke/`) | c63a1d725b5f3f3d5e956c7bec56da4d87beb73ac77e85d27e7c2f47df09347b |

## Re-extraction

    just clean <board>
    just build <board>
    cp target/<triple>/release/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin bootloaders/<chip>.bin
    sha256sum bootloaders/<chip>.bin
    # Update this README.
