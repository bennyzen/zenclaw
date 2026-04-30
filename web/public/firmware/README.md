Rust firmware artifacts for the web UI provisioning wizard.

Build with: ../../../scripts/build-rust-firmware.sh

Outputs:
- zenclaw-devkitc.bin     ESP32-S3 DevKitC (PSRAM)
- zenclaw-guition-p4.bin  Guition JC-ESP32P4-M3-DEV (Ethernet)
- firmware.json           Board manifest consumed by provision.vue
