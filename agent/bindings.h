// Top-level extra bindings for the agent crate.
// esp-idf-sys 0.37 ships a slimmer default bindings header that no longer
// pulls in esp_chip_info.h transitively, so we add it explicitly to expose
// esp_chip_info(), esp_chip_info_t, and esp_chip_model_t_* constants.
#include "esp_chip_info.h"
