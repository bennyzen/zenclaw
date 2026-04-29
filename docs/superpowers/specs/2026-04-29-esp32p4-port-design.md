# ESP32-P4 Port — Design Spec

**Date:** 2026-04-29
**Status:** Approved (brainstorm phase)
**Scope:** Port the existing Rust agent (`agent-esp32/`) to the Espressif ESP32-P4 architecture, targeting the Guition `JC-ESP32P4-M3-DEV` board over Ethernet, while preserving full feature parity with current ESP32-S3 builds and laying down build-system foundations for further board/target growth.

## Motivation

A new Guition `JC-ESP32P4-M3-DEV` board has joined the lab. It exposes capabilities the S3 doesn't have:

- **RISC-V dual-core @ 360 MHz** (vs Xtensa dual @ 240 MHz)
- **32 MB HEX-mode PSRAM** (vs 8 MB octal on DevKitC, 0 on T-Dongle)
- **10/100 Ethernet** via internal EMAC + IP101 PHY
- **MIPI-DSI / MIPI-CSI** display + camera connectors
- **ES8311 audio codec**, RS-485, microSD slot, USB 2.0 HS

Bringing zenclaw up on this board is the first step toward exploiting any of those — but more immediately, it forces the codebase to stop assuming a single chip family. Doing the multi-board refactor *now*, while we still have only two existing boards (DevKitC + T-Dongle), keeps the surface small. Every additional board added before the refactor will compound the cost.

The user has explicitly signalled that more boards and targets are coming, so the design favours a **manifest-driven, scalable** build system over the simplest thing that works for two chip families.

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Goal | Bar #3 — full feature parity with S3 build over Ethernet | Telegram + sessions + tools + memory all must work |
| Approach | Smoke-test first, then port | Surfaces toolchain papercuts in a 100-line program rather than a 1200-line `main.rs` |
| WiFi for v1 | Deferred to v2 (`esp_hosted` to ESP32-C6 over SDIO) | Ethernet is plugged in; C6 is the highest-risk subsystem; no rush |
| WiFi UI | Always compiled in regardless of WiFi driver presence | Dashboard stays consistent across boards; v2 lights up the C6 path additively |
| Build system | Justfile + per-board TOML manifests | Adding a new board = one new manifest + one new sdkconfig overlay; scales |
| Multi-arch in `.cargo/config.toml` | `cfg(target_arch="xtensa"/"riscv32")` blocks | Future RISC-V targets (C3/C5/C6) inherit linker/rustflags free |
| Bootloader strategy | Vendor pre-built bootloaders per chip family in `bootloaders/` | Mirrors existing S3 practice; provenance documented in `bootloaders/README.md` |
| Pin numbers (PHY, GPIO) | In Rust source, not sdkconfig | Greppable, type-checked, no opaque KConfig fragments |
| Smoke test | Permanent sibling crate `agent-esp32-smoke/` | Reusable as a "first-port reference" for future chip families |
| Out of scope (v1) | C6 WiFi, MIPI display, MIPI camera, ES8311 audio, microSD, USB Host MSC, RS-485 | Each adds variables; better to land Ethernet + agent first |

## Hardware Reference — Guition `JC-ESP32P4-M3-DEV`

Confirmed via vendor docs, ESPHome configuration, and `espflash board-info` against the unit in hand.

| Property | Value |
|---|---|
| Chip | ESP32-P4 rev v1.3 |
| Cores | 2× RISC-V @ 360 MHz (RV32IMAFC) |
| Flash | 16 MB |
| PSRAM | 32 MB (HEX SPI, external pin-controlled) |
| Console | USB JTAG/serial (native, `303a:1001`) — `/dev/ttyACM0` |
| Ethernet PHY | IP101 (RMII) |
| MAC of unit on hand | `80:f1:b2:d3:4c:09` |

### Ethernet pin map

| Signal | GPIO |
|---|---|
| MDC | 31 |
| MDIO | 52 |
| PHY power enable | 51 |
| RMII REF_CLK | 50 (input — external 50 MHz oscillator on board) |
| PHY address | 1 |

### ESP32-C6 co-processor (v2 only — held in reset for v1)

| Signal | GPIO |
|---|---|
| Reset | 54 (held LOW at boot in v1) |
| SDIO CMD | 19 |
| SDIO CLK | 18 |
| SDIO D0 | 14 |
| SDIO D1 | 15 |
| SDIO D2 | 16 |
| SDIO D3 | 17 |

## Project Structure (after this work)

```
zenclaw/
  agent-esp32/
    .cargo/config.toml             # cfg(target_arch=...) blocks, no [build] target
    rust-toolchain.toml            # union of all target triples
    Cargo.toml                     # nic-wifi-internal, nic-wifi-hosted, nic-eth features
    justfile                       # NEW — `just build/flash/monitor/list/clean <board>`
    partitions.csv                 # unchanged (16 MB layout)
    sdkconfig.defaults             # unchanged
    sdkconfig.board.devkitc        # unchanged
    sdkconfig.board.sdcard         # unchanged
    sdkconfig.board.guition-p4     # NEW
    boards/                        # NEW — manifest-driven board registry
      devkitc.toml
      sdcard.toml
      guition-p4.toml
    bootloaders/                   # NEW directory
      README.md                    # provenance: ESP-IDF version + sha256 per file
      esp32s3.bin                  # renamed from agent-esp32/bootloader.bin
      esp32p4.bin                  # extracted from first smoke build
    src/
      main.rs                      # WiFi init extracted; uses Nic trait
      lib.rs                       # `pub mod net;`
      led_status.rs                # WifiConnecting → LinkConnecting (NIC-agnostic)
      net/                         # NEW
        mod.rs                     # Nic trait, NicKind, bring_up_primary()
        eth.rs                     # IP101 EMAC bring-up, EthNic impl
        wifi.rs                    # extracted from main.rs (WifiNic impl)
        wifi_ui.rs                 # always-compiled NVS read/write + /api/wifi handlers
      …                            # core/, esp32/, platform/, etc. unchanged

  agent-esp32-smoke/               # NEW — sibling crate
    Cargo.toml                     # esp-idf-svc only, no zenclaw deps
    rust-toolchain.toml            # same target union
    .cargo/config.toml             # same cfg(target_arch=...) blocks
    justfile                       # same recipes
    partitions.csv                 # 4 MB factory + 1 MB nvs (no SPIFFS)
    sdkconfig.defaults
    sdkconfig.board.guition-p4     # mirrors agent-esp32 verbatim
    boards/guition-p4.toml
    bootloaders/esp32p4.bin
    src/main.rs                    # ~250 LoC, 6 numbered checkpoints
    README.md                      # what each step proves; how to fork for new chips

  CLAUDE.md                        # updated: just invocations, Guition row, bootloader notes
```

## Section 1 — Scope & Non-Goals

**In scope (v1):**
- New board profile `guition-p4` building for `riscv32imafc-esp-espidf`.
- Ethernet bring-up (IP101 PHY) replacing WiFi as the primary NIC.
- Same NVS layout, same partition layout, same SPIFFS for sessions/memory.
- Full HTTP API parity: `/api/status`, `/api/chat`, `/api/files/*`, `/api/config`, `/api/restart`, `/ws/chat`, `/ws/logs`.
- mDNS as `zenclaw.local` over Ethernet.
- Telegram long-poll + send.
- Sessions, tools, memory store — unchanged (target-agnostic).
- Multi-board build refactor (Justfile + manifest-driven), with both existing S3 boards migrated.

**Deferred to v2 (each becomes a follow-up spec):**
- WiFi via the C6 co-processor (`esp_hosted` over SDIO). Pins documented; C6 held in reset at boot.
- MIPI-DSI display panel.
- MIPI-CSI camera capture.
- ES8311 audio codec.
- microSD card mounting.
- USB Host MSC on Guition (different USB routing than DevKitC).
- RS-485.

**Explicit non-goals:**
- Changing S3 board behaviour. The S3 build path must regress-test green after the refactor.
- Replacing `agent-esp32-smoke/` with the next chip family — it's a *template* for future ports, not a one-shot.

## Section 2 — Toolchain & Multi-Board Build System

### `rust-toolchain.toml`

```toml
[toolchain]
channel = "esp"
components = ["rust-src"]
targets = ["xtensa-esp32s3-espidf", "riscv32imafc-esp-espidf"]
```

### `.cargo/config.toml`

```toml
# No [build] target — must be set via CARGO_BUILD_TARGET (justfile does this)

[target.'cfg(target_arch = "xtensa")']
linker = "ldproxy"
runner = "espflash flash --monitor"
rustflags = ["--cfg", "espidf_time64", "--cfg", "mio_unsupported_force_poll_poll"]

[target.'cfg(target_arch = "riscv32")']
linker = "ldproxy"
runner = "espflash flash --monitor"
rustflags = ["--cfg", "espidf_time64", "--cfg", "mio_unsupported_force_poll_poll"]

[unstable]
build-std = ["std", "panic_abort"]

[env]
ESP_IDF_VERSION = "v5.4"
```

Future RISC-V chip families (C3/C5/C6) inherit the RISC-V block automatically.

### `boards/<name>.toml` manifest format

```toml
# boards/guition-p4.toml
chip = "esp32p4"
target = "riscv32imafc-esp-espidf"
sdkconfig = ["sdkconfig.defaults", "sdkconfig.board.guition-p4"]
bootloader = "bootloaders/esp32p4.bin"
features = ["nic-eth"]
default_baud = 460800
description = "Guition JC-ESP32P4-M3-DEV (Ethernet, 32MB PSRAM, 16MB flash)"
```

```toml
# boards/devkitc.toml
chip = "esp32s3"
target = "xtensa-esp32s3-espidf"
sdkconfig = ["sdkconfig.defaults", "sdkconfig.board.devkitc"]
bootloader = "bootloaders/esp32s3.bin"
features = ["nic-wifi-internal"]
default_baud = 921600
description = "ESP32-S3-DevKitC (PSRAM, USB Host capable)"
```

```toml
# boards/sdcard.toml
chip = "esp32s3"
target = "xtensa-esp32s3-espidf"
sdkconfig = ["sdkconfig.defaults", "sdkconfig.board.sdcard"]
bootloader = "bootloaders/esp32s3.bin"
features = ["nic-wifi-internal"]
default_baud = 921600
description = "LILYGO T-Dongle-S3 (no PSRAM, SD slot)"
```

### `justfile` recipes

```just
set shell := ["bash", "-cu"]

# List supported boards with descriptions
list:
    #!/usr/bin/env bash
    echo "Available boards:"
    for f in boards/*.toml; do
        name=$(basename "$f" .toml)
        desc=$(awk -F' = ' '/^description/ {print $2}' "$f" | tr -d '"')
        printf "  %-15s %s\n" "$name" "$desc"
    done

# Build firmware for a board: `just build guition-p4 [extra cargo args]`
build board *args="--release":
    #!/usr/bin/env bash
    set -euo pipefail
    eval "$(scripts/board-env.sh {{board}})"
    CARGO_BUILD_TARGET="$TARGET" \
    ESP_IDF_SDKCONFIG_DEFAULTS="$SDKCONFIG" \
        cargo build --features "$FEATURES" {{args}}

# Flash to device: `just flash guition-p4 [/dev/ttyACM0]`
flash board port="/dev/ttyACM0":
    #!/usr/bin/env bash
    set -euo pipefail
    eval "$(scripts/board-env.sh {{board}})"
    espflash flash --port {{port}} \
        --partition-table partitions.csv \
        --bootloader "$BOOTLOADER" \
        --baud "$BAUD" \
        "target/$TARGET/release/zenclaw-agent"

# Stream serial console
monitor port="/dev/ttyACM0":
    espflash monitor --port {{port}} --non-interactive

# Wipe build cache for a specific board (use after switching architectures)
clean board:
    #!/usr/bin/env bash
    eval "$(scripts/board-env.sh {{board}})"
    rm -rf "target/$TARGET/release/build/esp-idf-sys-"*
```

`scripts/board-env.sh <board>` is a tiny helper that emits shell `export` lines parsed from the manifest TOML. Keeps the justfile readable; if TOML parsing in bash gets ugly, swap for Python without changing the recipe surface.

### `Cargo.toml` features

```toml
[features]
default = ["esp32", "nic-wifi-internal"]
esp32 = ["dep:esp-idf-svc", "dep:embedded-svc"]
desktop = [...]                          # unchanged

# NIC drivers — chosen per board manifest
nic-wifi-internal = ["esp32"]            # S3/S2 native WiFi via EspWifi
nic-wifi-hosted   = ["esp32"]            # WiFi via esp_hosted (C6/C5) — v2
nic-eth           = ["esp32"]            # Internal EMAC + external PHY (P4)

usb_storage = ["esp32"]                  # unchanged
hnsw = ["dep:usearch"]                   # unchanged (gated on Xtensa if RISC-V build fails)
```

The default features cover the common interactive case (`cargo build` for S3 + WiFi), so the existing developer flow is unchanged for users who don't adopt `just`.

## Section 3 — sdkconfig for Guition P4

`agent-esp32/sdkconfig.board.guition-p4`:

```
# Target
CONFIG_IDF_TARGET="esp32p4"

# Console (P4 has native USB Serial/JTAG)
CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y
# CONFIG_ESP_CONSOLE_UART_DEFAULT is not set

# PSRAM — 32MB HEX-mode at 200MHz on Guition
CONFIG_SPIRAM=y
CONFIG_SPIRAM_MODE_HEX=y
CONFIG_SPIRAM_SPEED_200M=y
CONFIG_SPIRAM_USE_MALLOC=y
CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=16384
CONFIG_SPIRAM_TRY_ALLOCATE_WIFI_LWIP=y

# Ethernet via internal EMAC + IP101 PHY (external 50MHz REF_CLK input)
CONFIG_ETH_USE_ESP32_EMAC=y
CONFIG_ETH_PHY_INTERFACE_RMII=y
CONFIG_ETH_RMII_CLK_INPUT=y
CONFIG_ETH_RMII_CLK_IN_GPIO=50

# WiFi/BT disabled (no internal radio; C6 deferred to v2)
# CONFIG_ESP_WIFI_ENABLED is not set
# CONFIG_BT_ENABLED is not set
```

Pin-level Ethernet config (MDC/MDIO/PHY power/PHY addr) lives in Rust source (`net/eth.rs`), not in sdkconfig — discoverable, greppable, type-checked.

If `CONFIG_SPIRAM_SPEED_200M` proves unstable on this Guition unit (R2 in risk register), fall back to `CONFIG_SPIRAM_SPEED_80M`. Smoke test step 2 includes a 16 MB pattern test that catches PSRAM corruption.

### Bootloader provenance

ESP-IDF builds a chip-family-specific bootloader as a side-effect of building the app. Procedure:

1. Clean build for `guition-p4` (smoke crate).
2. Copy `target/riscv32imafc-esp-espidf/release/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin` to:
   - `agent-esp32-smoke/bootloaders/esp32p4.bin`
   - `agent-esp32/bootloaders/esp32p4.bin`
3. Compute sha256 and log it in `bootloaders/README.md` of both crates with the ESP-IDF version (`v5.4`) and date.

Same procedure was used originally for `esp32s3.bin`.

## Section 4 — Network Layer (Ethernet alongside WiFi UI)

### `Nic` trait

```rust
pub enum NicKind { Wifi, Ethernet }

pub trait Nic: Send + Sync {
    fn kind(&self) -> NicKind;
    fn link_up(&self) -> bool;
    fn ip_info(&self) -> Option<IpInfo>;
    fn link_speed_mbps(&self) -> Option<u32>;
    fn ssid(&self) -> Option<String>;          // None for Ethernet
    fn rssi(&self) -> Option<i32>;             // None for Ethernet
    fn mac(&self) -> [u8; 6];
}

pub fn bring_up_primary(/* peripherals, sysloop, nvs */) -> anyhow::Result<Box<dyn Nic>> {
    #[cfg(feature = "nic-eth")]              { return eth::bring_up(/* ... */); }
    #[cfg(feature = "nic-wifi-internal")]    { return wifi::bring_up(/* ... */); }
    #[cfg(feature = "nic-wifi-hosted")]      { return wifi::bring_up_hosted(/* ... */); }
    #[cfg(not(any(feature = "nic-wifi-internal", feature = "nic-wifi-hosted", feature = "nic-eth")))]
    compile_error!("at least one NIC feature must be enabled");
}
```

main.rs becomes:

```rust
let nic = net::bring_up_primary(peripherals, sysloop.clone(), nvs.clone())?;
log::info!("Primary NIC: {:?} ip={:?}", nic.kind(), nic.ip_info());
// HTTP server, mDNS, Telegram all bind 0.0.0.0 — no NIC awareness needed.
//
// /api/wifi handlers and the wifi.* block in /api/status are registered
// unconditionally below — they read from NVS and from the active NIC (if any),
// so they work uniformly across all board feature combinations.
```

When multiple NIC features are enabled (e.g. v2 Guition with `nic-eth` + `nic-wifi-hosted`), `bring_up_primary` follows the cfg ordering above: Ethernet first, then internal WiFi, then hosted WiFi. The first available driver becomes the *primary* NIC for `/api/status.network`. Secondary NIC bring-up is a v2 concern and not part of this spec.

### WiFi UI is always present

`net/wifi_ui.rs` is **not** feature-gated. It exposes:

- `read_credentials(&EspNvsPartition) -> Option<(String, Option<String>)>`
- `write_credentials(&EspNvsPartition, ssid, password) -> Result<()>`
- HTTP handlers for `/api/wifi` GET/PUT

On boards without an active WiFi driver (Guition v1), PUT still saves credentials to NVS — they wait for v2 (`nic-wifi-hosted`) to pick them up. GET reports stored SSID and `driver: "none"`.

### `/api/status` JSON evolution

```json
{
  "network": {
    "kind": "ethernet",
    "ip": "192.168.1.42",
    "link_speed_mbps": 100,
    "mac": "80:f1:b2:d3:4c:09"
  },
  "wifi": {
    "connected": false,
    "ip": null,
    "ssid": "MySavedSSID",
    "rssi": null,
    "driver": "none"
  },
  "...": "heap, psram, version, channels, etc. unchanged"
}
```

`network.*` is the canonical primary-NIC report going forward. `wifi.*` is preserved as the existing dashboard contract; on Ethernet-only boards, `wifi.driver = "none"` tells the UI to render a "WiFi credentials saved but no WiFi driver in this build" hint.

### Cosmetic rename

`led_status::State::WifiConnecting` → `LinkConnecting`, `WifiFailed` → `LinkFailed`. NIC-agnostic.

## Section 5 — Memory & PSRAM

Mostly headroom, not new code.

| Resource | DevKitC (S3) | Guition (P4) | Effect |
|---|---|---|---|
| Internal SRAM | ~512 KB | ~768 KB | More stack/DMA budget |
| PSRAM | 8 MB | 32 MB | mbedtls, lwIP, sessions all comfortable |
| PSRAM bus | OCT (8-bit) | HEX (16-bit) | ~2× memory bandwidth |
| Free heap after WiFi+TLS+HTTPD | ~2-3 MB | ~25-28 MB est. | No memory-pressure compaction needed |

Heap policy unchanged: `CONFIG_SPIRAM_USE_MALLOC=y` + `CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=16384`. Stack sizes left at defaults. DMA descriptors land in internal SRAM automatically (lwIP and esp-eth use `MALLOC_CAP_INTERNAL`/`MALLOC_CAP_DMA` correctly).

`/api/status` heap/PSRAM reporting works identically (`heap_caps_get_*` and `esp_psram_get_size` are chip-agnostic) — verify symbol availability in smoke test, treat as a non-issue otherwise.

## Section 6 — Smoke Test (`agent-esp32-smoke/`)

Permanent sibling crate. Validates platform layer before agent migration. Self-contained, zero zenclaw imports, ~250 LoC. Stays in-tree as a "first-port reference" for future chip families.

### Six numbered checkpoints

| # | Step | Validates |
|---|---|---|
| 1 | `chip_info` — print model/revision/cores | RISC-V `esp` toolchain compiles & links; bootloader chain-loads; ESP-IDF v5.4 starts on P4 |
| 2 | `psram` — detect size + 16 MB pattern test | sdkconfig HEX/200M flags match the silicon; allocator picks PSRAM up |
| 3 | `ethernet_link` — link UP + speed | IP101 wiring (MDC=31, MDIO=52, PWR=51, CLK_IN=50, addr=1) correct |
| 4 | `dhcp` — IP/gw/dns | lwIP + DHCP + DNS up; netif registered |
| 5 | `outbound_https` — `GET https://httpbin.org/ip` | mbedtls + esp-tls compile & work on RISC-V (canary for Telegram + LLM API calls) |
| 6 | `inbound_http` — bind `0.0.0.0:80` + `/ping` | esp_http_server compiles & binds |

Console output:

```
[1/6] chip_info.........: ESP32-P4 rev v1.3, 2 cores @ 360MHz
[2/6] psram.............: 32 MiB detected, 31987 KiB free heap
[3/6] ethernet_link.....: link UP @ 100 Mbps, MAC 80:f1:b2:d3:4c:09
[4/6] dhcp..............: ip=192.168.1.42  gw=192.168.1.1  dns=192.168.1.1
[5/6] outbound_https....: GET https://httpbin.org/ip → 200 (32 bytes)
[6/6] inbound_http......: server listening on :80, try GET /ping
SMOKE PASS — toolchain, bootloader, PSRAM, EMAC+IP101, lwIP, mbedtls, httpd all OK
```

Pass criteria: all six PASS *and* `curl http://<device-ip>/ping` returns `pong`.

Step-to-fix mapping in case of failure: 1→toolchain/espup, 2→sdkconfig PSRAM flags, 3→PHY pin map, 4→network/cable/DHCP, 5→mbedtls/esp-tls, 6→http server.

## Section 7 — File-Structure Change Summary

**New files (~700 LoC):**

| Path | Purpose | Approx LoC |
|---|---|---|
| `agent-esp32/justfile` | Build/flash/monitor/list/clean recipes | ~80 |
| `agent-esp32/scripts/board-env.sh` | TOML manifest → shell exports | ~30 |
| `agent-esp32/boards/devkitc.toml` | Board manifest | ~12 |
| `agent-esp32/boards/sdcard.toml` | Board manifest | ~12 |
| `agent-esp32/boards/guition-p4.toml` | New board manifest | ~12 |
| `agent-esp32/sdkconfig.board.guition-p4` | P4 + IP101 + HEX-PSRAM ESP-IDF flags | ~25 |
| `agent-esp32/bootloaders/README.md` | Bootloader provenance | ~20 |
| `agent-esp32/bootloaders/esp32p4.bin` | Vendored P4 bootloader | binary |
| `agent-esp32/src/net/mod.rs` | Nic trait, NicKind, bring_up_primary | ~80 |
| `agent-esp32/src/net/eth.rs` | IP101 EMAC bring-up + EthNic | ~120 |
| `agent-esp32/src/net/wifi.rs` | Extracted WiFi driver + WifiNic | ~100 |
| `agent-esp32/src/net/wifi_ui.rs` | Always-compiled NVS + /api/wifi handlers | ~80 |
| `agent-esp32-smoke/` (whole crate) | Standalone P4 validation | ~250 |

**Modified files (~150 LoC moved/refactored):**

| Path | Change |
|---|---|
| `agent-esp32/.cargo/config.toml` | Drop `[build] target`; replace per-triple blocks with `cfg(target_arch=...)` blocks |
| `agent-esp32/rust-toolchain.toml` | `targets = ["xtensa-esp32s3-espidf", "riscv32imafc-esp-espidf"]` |
| `agent-esp32/Cargo.toml` | New features `nic-wifi-internal`, `nic-wifi-hosted`, `nic-eth`; default updated; verify `esp-idf-svc` `eth` feature |
| `agent-esp32/src/main.rs` | Extract WiFi to `net::wifi`; `let nic = net::bring_up_primary(...)`; add `/api/status.network`; rename LED states; remove `get_wifi_info()` |
| `agent-esp32/src/lib.rs` | `pub mod net;` |
| `agent-esp32/src/led_status.rs` | `WifiConnecting` → `LinkConnecting`; `WifiFailed` → `LinkFailed` |
| `CLAUDE.md` | `just <recipe>` invocations; Guition row in board table; document `agent-esp32-smoke/`; vendored bootloaders directory |

**Renamed:**

| Old | New |
|---|---|
| `agent-esp32/bootloader.bin` | `agent-esp32/bootloaders/esp32s3.bin` (`git mv` to preserve history) |

**No deletions of working code.** Web UI untouched (existing `wifi.*` field preserved with `driver: "none"` semantic on Ethernet-only boards).

## Section 8 — Risks, Open Questions, Verification

### Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | mbedtls / esp-tls fails to build for `riscv32imafc-esp-espidf` | Low | Blocks port | Smoke step 5; escalate `esp-idf-svc` minor if hit |
| R2 | HEX PSRAM at 200 MHz unstable on this Guition unit | Medium | Heap corruption | Smoke step 2 pattern test; fall back to `SPIRAM_SPEED_80M` |
| R3 | IP101 PHY pin map differs from ESPHome's (revision drift) | Low | Step 3 fails | Falsifiable in 5 min of pin probing |
| R4 | `--cfg espidf_time64` flag mismatch for RISC-V | Medium | Build error / `time_t` ABI mismatch | Smoke crate uses identical rustflags; fails at compile |
| R5 | Vendored P4 bootloader incompatibility | Low | Boot loop | First boot uses bootloader extracted from same build (provenance-locked) |
| R6 | C6 factory firmware emits RF / draws current | Low | Cosmetic | Hold C6 in reset via GPIO54-LOW at boot |
| R7 | Cargo target switch leaves stale `esp-idf-sys` build cache | High | Mysterious link errors | `just clean <board>` recipe; CLAUDE.md callout |
| R8 | `just` not installed on user's system | High | Blocks first build | One-line install in `agent-esp32/README.md` |
| R9 | `esp-idf-svc 0.51` needs `eth` feature flag | Medium | Compile error in `net/eth.rs` | Verify in Cargo.toml during implementation |
| R10 | `usearch` (hnsw feature) doesn't compile for RISC-V | Low | Affects opt-in feature only | Gate on `not(target_arch="riscv32")` if hit |
| R11 | Dashboard reads strict types; missing fields confuse UI | Low | UX regression | `wifi.driver = "none"` is additive only; no removed fields |
| R12 | DHCP fails on the user's network | Medium | Smoke step 4 fails | Add `SMOKE_STATIC_IP=...` env-var fallback |

### Open questions (resolved during implementation, not now)

- **Q1** — Does `esp-idf-svc 0.51` expose `RmiiPhy::IP101` directly, or do we need to drop to the C symbol `esp_eth_phy_new_ip101` via FFI? (~10 LoC delta in `net/eth.rs`.)
- **Q2** — Exact ESP-IDF patch version pulled in for P4 — `v5.4` is declared, resolver may pick a slightly different patch. Logged at first build.
- **Q3** — Does the C6 reset pin (GPIO54) need an external pull-up for v1, or is the internal pull-down sufficient to keep it in reset while P4 boots?
- **Q4** — Does `espflash` 4.x handle P4 partition tables identically to S3? (Format same; chip-specific defaults may differ.)
- **Q5** — Status LED pin on Guition (ESPHome suggests GPIO45 — needs board-side verification before wiring `LinkConnecting`/`LinkFailed` LED states).
- **Q6** — `CONFIG_ETH_USE_ESP32_EMAC` symbol name on ESP-IDF v5.4 for P4 target. The S3-era name may have been renamed to `CONFIG_ETH_USE_ESP32P4_EMAC` or a target-conditional alias. Verified by the smoke build's first `idf.py menuconfig` resolution.

### Verification plan

**Phase A — Smoke test (validates platform layer):**
1. `cd agent-esp32-smoke && just build guition-p4` produces a binary
2. `just flash guition-p4 /dev/ttyACM0` flashes without error
3. Serial console shows all 6 checkpoints `PASS`
4. `curl http://<device-ip>/ping` from host returns `pong`
5. Bootloader extracted from build, sha256 logged in `bootloaders/README.md`

**Phase B — Multi-board build system (validates Section 2 refactor):**
6. `cd agent-esp32 && just list` shows all three boards with descriptions
7. `just build devkitc` produces a working S3 binary (regression)
8. `just build sdcard` builds (regression)
9. `just build guition-p4` builds
10. `just flash devkitc` works on a real S3 board (regression)

**Phase C — Agent port (validates Sections 3-5 + everything):**
11. `just flash guition-p4 /dev/ttyACM0` flashes the agent
12. Boots, mDNS resolves `zenclaw.local`
13. `curl http://zenclaw.local/api/status | jq` returns valid JSON: `network.kind="ethernet"`, `network.ip` populated, `wifi.driver="none"`, plus heap/PSRAM/version
14. `curl -X POST http://zenclaw.local/api/chat -d '{"message":"ping"}'` returns an LLM reply (proves outbound HTTPS, sessions, agent loop end-to-end)
15. Telegram bot works (send → reply)
16. `/api/files/*` works against SPIFFS
17. Soft reboot via `/api/restart` — config persists, agent comes back up
18. **S3 regression**: same battery against a DevKitC build — `/api/chat` works there too

**Phase D — Documentation:**
19. CLAUDE.md updated: Guition row, `just` invocations, multi-arch caveats
20. `bootloaders/README.md` documents provenance for both bootloaders
21. `agent-esp32-smoke/README.md` documents what it proves and how to fork for the next chip family

### Stopping conditions (v1 done when all of these are true)

- Phases A + B + C all pass on the Guition unit currently plugged in
- DevKitC regression passes (real hardware preferred; clean build at minimum)
- Auto-memory updated with the Guition board entry + first-boot date

## Future Work (out of scope here, listed for context)

| Item | Trigger |
|---|---|
| C6 WiFi via `esp_hosted` SDIO | When WiFi mobility on Guition is needed |
| MIPI-DSI display | When local UI is needed (kiosk, dashboard) |
| MIPI-CSI camera | When vision tools are wanted |
| ES8311 audio | When voice input/output is wanted |
| microSD mount | When session/memory > 8 MB SPIFFS |
| Telegram-on-RISC-V perf profile | After v1 lands; baseline comparison vs S3 |
| Additional RISC-V boards (C5, C6, NANO) | As they arrive — re-use smoke crate template |
