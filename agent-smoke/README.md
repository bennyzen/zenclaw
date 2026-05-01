# agent-esp32-smoke

Standalone ESP-IDF + Rust smoke-test for new chip families. Validates that the
toolchain, vendored bootloader, PSRAM, NIC, lwIP, mbedTLS, and esp_http_server
all work on a fresh hardware target before bringing up the full agent.

Use as a **template** when porting to a new chip family — copy this crate, swap
pins/sdkconfig in `boards/<name>.toml` and `sdkconfig.board.<name>`, re-run.

## Usage

    just list                               # show supported boards
    just build guition-p4                   # build for a board
    just flash guition-p4 /dev/ttyACM0      # flash
    just monitor /dev/ttyACM0               # serial console

## What each checkpoint proves

| # | Step | Validates |
|---|---|---|
| 1 | chip_info | toolchain compiles & links; bootloader chain-loads; ESP-IDF starts on this chip |
| 2 | psram | sdkconfig PSRAM mode/speed flags match the silicon; allocator picks it up |
| 3 | ethernet_link | PHY pin map and config correct |
| 4 | dhcp | lwIP + DHCP + DNS up; netif registered |
| 5 | outbound_https | mbedtls + esp-tls compiled and works (Telegram + LLM API canary) |
| 6 | inbound_http | esp_http_server compiled and binds (HTTP API canary) |

If any step fails, fix that subsystem before proceeding to the agent port.
