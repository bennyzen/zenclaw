idf_component_register(
    SRCS "zenclaw_sd.c"
    INCLUDE_DIRS "include"
    REQUIRES esp_driver_sdmmc fatfs sdmmc vfs log driver
)
