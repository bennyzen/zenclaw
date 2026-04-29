// Extra esp-idf headers we want bindgen to expose.
// esp-idf-sys 0.37+ no longer pulls esp_chip_info.h transitively from
// esp_system.h, so request it explicitly here.
#include "esp_chip_info.h"
// PSRAM detection and heap_caps for checkpoint 2.
#include "esp_psram.h"
#include "esp_heap_caps.h"
