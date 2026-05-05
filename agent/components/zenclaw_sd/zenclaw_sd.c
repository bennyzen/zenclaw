#include "zenclaw_sd.h"

#include "esp_vfs_fat.h"
#include "driver/sdmmc_host.h"
#include "sdmmc_cmd.h"
#include "sd_protocol_defs.h"
#include "esp_log.h"
#include "ff.h"
#include "soc/soc_caps.h"
#if SOC_SDMMC_IO_POWER_EXTERNAL
#include "sd_pwr_ctrl_by_on_chip_ldo.h"
#endif

static const char *TAG = "zenclaw_sd";
#define MOUNT_POINT "/sdcard"

// On ESP32-P4 the SDMMC IO_MUX pins (slot 0) get their bus voltage from the
// chip's on-chip LDO_VO4 regulator — without configuring it, the SD slot
// has no IO voltage and cards never respond (CMD0 → ESP_ERR_TIMEOUT).
// LDO channels 1 & 2 are reserved (flash & PSRAM); channel 4 is the
// vendor-recommended default for P4 boards including the Espressif
// EV-Board, and we assume the Guition matches.
#define ZENCLAW_SD_LDO_CHAN_ID 4

static sdmmc_card_t *s_card = NULL;
static int s_bus_width = 0;
#if SOC_SDMMC_IO_POWER_EXTERNAL
static sd_pwr_ctrl_handle_t s_pwr_ctrl = NULL;
#endif

static esp_err_t mount_with_width(int width) {
    // SDMMC_HOST_DEFAULT() defaults to slot 1 (GPIO-matrix); we want slot 0
    // (IO_MUX dedicated pins) for max throughput on the P4. The slot_config
    // pin assignments are ignored on slot 0 (pins are fixed in IO_MUX), but
    // we set them via SDMMC_SLOT_CONFIG_DEFAULT() anyway for clarity and so
    // this code is portable to slot-1 boards in the future.
    sdmmc_host_t host = SDMMC_HOST_DEFAULT();
    host.slot = SDMMC_HOST_SLOT_0;
#if SOC_SDMMC_IO_POWER_EXTERNAL
    host.pwr_ctrl_handle = s_pwr_ctrl;
#endif

    sdmmc_slot_config_t slot_config = SDMMC_SLOT_CONFIG_DEFAULT();
    slot_config.width = width;
    // Internal pull-ups protect against floating lines on bare slots; some
    // breakouts already pull up externally — having both is harmless.
    slot_config.flags |= SDMMC_SLOT_FLAG_INTERNAL_PULLUP;

    // format_if_mount_failed=false is critical: SD cards carry user data,
    // often pre-loaded from a desktop. Auto-format would silently destroy it.
    esp_vfs_fat_mount_config_t mount_config = {
        .format_if_mount_failed = false,
        .max_files = 8,
        .allocation_unit_size = 0,
        .disk_status_check_enable = false,
        .use_one_fat = false,
    };

    return esp_vfs_fat_sdmmc_mount(MOUNT_POINT, &host, &slot_config,
                                   &mount_config, &s_card);
}

esp_err_t zenclaw_sd_mount(void) {
    if (s_card != NULL) {
        return ESP_ERR_INVALID_STATE;
    }

#if SOC_SDMMC_IO_POWER_EXTERNAL
    // Bring up the on-chip LDO that powers the SDMMC IO bus. Without this
    // on P4, every CMD times out (ESP_ERR_TIMEOUT 0x107).
    if (s_pwr_ctrl == NULL) {
        sd_pwr_ctrl_ldo_config_t ldo_cfg = {
            .ldo_chan_id = ZENCLAW_SD_LDO_CHAN_ID,
        };
        esp_err_t lret = sd_pwr_ctrl_new_on_chip_ldo(&ldo_cfg, &s_pwr_ctrl);
        if (lret != ESP_OK) {
            ESP_LOGE(TAG, "sd_pwr_ctrl_new_on_chip_ldo(chan=%d) failed (0x%x)",
                     ZENCLAW_SD_LDO_CHAN_ID, lret);
            s_pwr_ctrl = NULL;
            return lret;
        }
        ESP_LOGI(TAG, "on-chip LDO_VO%d configured for SDMMC IO power",
                 ZENCLAW_SD_LDO_CHAN_ID);
    }
#endif

    esp_err_t ret = mount_with_width(4);
    if (ret == ESP_OK) {
        s_bus_width = 4;
        ESP_LOGI(TAG, "mounted at %s (4-bit, %dkHz)",
                 MOUNT_POINT, s_card->real_freq_khz);
        return ESP_OK;
    }
    ESP_LOGW(TAG, "4-bit mount failed (0x%x), trying 1-bit", ret);

    ret = mount_with_width(1);
    if (ret == ESP_OK) {
        s_bus_width = 1;
        ESP_LOGI(TAG, "mounted at %s (1-bit, %dkHz)",
                 MOUNT_POINT, s_card->real_freq_khz);
        return ESP_OK;
    }

    ESP_LOGE(TAG, "mount failed (0x%x); /sdcard unavailable", ret);
    s_card = NULL;
    s_bus_width = 0;
#if SOC_SDMMC_IO_POWER_EXTERNAL
    if (s_pwr_ctrl != NULL) {
        sd_pwr_ctrl_del_on_chip_ldo(s_pwr_ctrl);
        s_pwr_ctrl = NULL;
    }
#endif
    return ret;
}

bool zenclaw_sd_is_mounted(void) {
    return s_card != NULL;
}

esp_err_t zenclaw_sd_info(uint64_t *total_bytes, uint64_t *free_bytes) {
    if (s_card == NULL) return ESP_ERR_INVALID_STATE;
    if (total_bytes == NULL || free_bytes == NULL) return ESP_ERR_INVALID_ARG;

    // The SD card is the only FATFS volume in this build (LittleFS owns
    // /data) so drive number 0 is unambiguous. If a second FATFS volume is
    // ever added we'll need to track the pdrv from mount instead.
    FATFS *fs = NULL;
    DWORD free_clusters = 0;
    FRESULT res = f_getfree("0:", &free_clusters, &fs);
    if (res != FR_OK || fs == NULL) {
        ESP_LOGW(TAG, "f_getfree failed (%d)", res);
        return ESP_FAIL;
    }

    uint64_t bytes_per_cluster = (uint64_t)fs->csize * (uint64_t)fs->ssize;
    *total_bytes = (uint64_t)(fs->n_fatent - 2) * bytes_per_cluster;
    *free_bytes  = (uint64_t)free_clusters * bytes_per_cluster;
    return ESP_OK;
}

const char *zenclaw_sd_type(void) {
    if (s_card == NULL) return "none";
    if (s_card->is_sdio) return "SDIO";
    if (s_card->is_mmc) return "MMC";
    if (s_card->ocr & SD_OCR_SDHC_CAP) return "SDHC";
    return "SDSC";
}

int zenclaw_sd_bus_width(void) {
    return s_bus_width;
}
