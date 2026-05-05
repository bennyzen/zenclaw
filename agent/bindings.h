// Top-level extra bindings for the agent crate.
// esp-idf-sys 0.37 ships a slimmer default bindings header that no longer
// pulls in esp_chip_info.h transitively, so we add it explicitly to expose
// esp_chip_info(), esp_chip_info_t, and esp_chip_model_t_* constants.
#include "esp_chip_info.h"

// SD card (FATFS over SDMMC peripheral). Pulls esp_vfs_fat_sdmmc_mount,
// the sdmmc_host_t / sdmmc_slot_config_t types, and the underlying
// sdmmc_host_* function pointer targets. Headers are cheap to include
// even on boards without the `sdcard` cargo feature — bindgen only emits
// declarations, not code. The zenclaw_sd helper exposes the flat API the
// Rust agent calls (mount, info, type) — the component itself is at
// agent/components/zenclaw_sd.
#include "esp_vfs_fat.h"
#include "driver/sdmmc_host.h"
#include "sdmmc_cmd.h"
#include "zenclaw_sd.h"
