# Contributing to ZenClaw

Thanks for your interest in ZenClaw. This guide covers building, running, and
testing the project. For deeper architecture notes, board profiles, and known
pitfalls, see [`CLAUDE.md`](CLAUDE.md).

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE).

## Layout

- `agent/` — the single Rust crate. One codebase, three targets selected by
  cargo features: ESP32-S3 / ESP32-P4 (the `esp32` feature, default) and a
  native **desktop** build for development (the `desktop` feature).
- `agent-smoke/` — minimal reference crate for bringing up new chips.
- `web/` — Nuxt PWA (dashboard, config editor, file manager, provisioning).
- `docs/` — design docs and plans.

## Run locally (desktop) — no hardware required

The fastest way to hack on the agent core is the native desktop build. It runs
the same `Gateway`, tools, sessions, and HTTP API as the firmware, talking to
LLM providers via the `genai` crate over an axum server.

```bash
cd agent
cargo +stable run --no-default-features --features desktop
```

> `agent/rust-toolchain.toml` pins the Espressif `esp` toolchain for firmware
> builds, so it overrides every `cargo` call in `agent/`. The `+stable` prefix
> runs the desktop build on your default stable Rust instead — so you can hack on
> the agent core **without installing `espup`** (firmware builds still need it,
> see below).

- It reads **`config.json` from the current working directory** (`agent/config.json`).
  Create one with at least `agent_name`, `providers.default`, and a provider
  entry with `api_key` + `model` — see the [Configuration](README.md#configuration)
  section for the shape. This file is gitignored; never commit real keys.
- The API listens on `0.0.0.0:8080` by default (override with the `ZENCLAW_PORT`
  env var). The web UI can connect to it exactly like a physical device.

### Tests

```bash
cd agent
cargo +stable test --no-default-features --features desktop
```

Run a single test by appending its name (e.g. `cargo +stable test --no-default-features --features desktop sigv4`).

## Build & flash firmware (ESP32)

Firmware builds require the Espressif Rust toolchain. Install it with
[`espup`](https://github.com/esp-rs/espup):

```bash
cargo install espup
espup install
# then source the exported env in each shell:
. $HOME/export-esp.sh
```

> **Toolchain pin (Xtensa / ESP32-S3):** releases `1.94.0.0`–`1.95.0.0` hit an
> LLVM ICE building the S3 target. If you see `XtensaISD::PCREL_WRAPPER`, pin the
> toolchain: `espup install --toolchain-version 1.93.0.0`. The P4 (RISC-V) target
> is unaffected. See `CLAUDE.md` → Common Pitfalls.

The build system is [`just`](https://github.com/casey/just) + `espflash`:

```bash
cd agent
just list                          # show all boards
just build devkitc                 # ESP32-S3 DevKitC
just build guition-p4              # Guition ESP32-P4
just flash devkitc /dev/ttyACM0    # bootloader is selected automatically
just clean devkitc                 # wipe the esp-idf-sys cache for that target
```

The board profile **must** match your hardware (PSRAM, console, NIC). Flashing
the wrong profile can crash at boot — see `CLAUDE.md` → Board Profiles.

## Web UI

```bash
cd web
npm install        # Node 24+ recommended (matches CI)
npm run dev        # dev server at http://localhost:3000
```

Connect the dashboard to a running device (or the desktop build) by hostname.
To rebuild the firmware artifacts the provisioning wizard flashes:

```bash
./scripts/build-rust-firmware.sh           # all boards + firmware.json
./scripts/build-rust-firmware.sh devkitc   # one board
```

## Pull requests

- Keep changes focused; match the style and conventions of the surrounding code.
- Run `cargo test --no-default-features --features desktop` (and `cargo build`
  for any target you touched) before opening a PR.
- For security-sensitive reports, follow [`SECURITY.md`](SECURITY.md) instead of
  opening a public issue.
