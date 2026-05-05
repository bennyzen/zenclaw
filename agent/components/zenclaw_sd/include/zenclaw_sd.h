// SD card driver glue for ZenClaw — wraps ESP-IDF's SDMMC + FATFS layers
// behind a flat C API the Rust agent can call without hand-mirroring the
// SDMMC_HOST_DEFAULT() macro's 17 function pointers + anonymous union.
//
// The actual mount lives in zenclaw_sd.c. Rust calls zenclaw_sd_mount()
// once at boot; failures are non-fatal (the rest of the agent runs with
// /sdcard absent and `mounted: false` reported in /api/status.sdcard).

#pragma once

#include "esp_err.h"
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Try to mount the SD card at /sdcard via the SDMMC peripheral on slot 0
// (IO_MUX path — fixed pins on each chip). Tries 4-bit width first; on
// failure falls back to 1-bit so boards that only wire D0 still work.
//
// Returns ESP_OK on success. On failure returns the underlying esp_err_t
// (often ESP_ERR_TIMEOUT or ESP_ERR_NOT_FOUND); /sdcard remains unavailable
// and zenclaw_sd_is_mounted() returns false.
//
// Calling twice returns ESP_ERR_INVALID_STATE.
esp_err_t zenclaw_sd_mount(void);

// True if a card was successfully mounted at /sdcard.
bool zenclaw_sd_is_mounted(void);

// Filesystem capacity. Fills *total_bytes / *free_bytes via FATFS f_getfree.
// Returns ESP_OK on success, ESP_ERR_INVALID_STATE if not mounted, ESP_FAIL
// on FATFS error.
esp_err_t zenclaw_sd_info(uint64_t *total_bytes, uint64_t *free_bytes);

// Card type string: "SDHC" / "SDSC" / "MMC" / "SDIO" / "none". Always
// returns a non-NULL static C string.
const char *zenclaw_sd_type(void);

// Bus width currently in use (1 or 4). Returns 0 if not mounted.
int zenclaw_sd_bus_width(void);

#ifdef __cplusplus
}
#endif
