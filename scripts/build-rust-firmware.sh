#!/usr/bin/env bash
# Build merged Rust firmware images for the web UI provisioning wizard.
#
# Output: web/public/firmware/zenclaw-<board>.bin (one per board)
#         web/public/firmware/firmware.json (board manifest)
#
# Requires: just, espflash 3.x, the Xtensa Rust toolchain pinned to 1.93.0.0
# (see CLAUDE.md → S3 Xtensa LLVM bug pitfall).
#
# Usage: ./scripts/build-rust-firmware.sh [board1 board2 ...]
#        Defaults to all supported boards if none specified.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
AGENT_DIR="$REPO_ROOT/agent-esp32"
OUTPUT_DIR="$REPO_ROOT/web/public/firmware"

cd "$AGENT_DIR"

declare -a BOARDS
if [[ $# -gt 0 ]]; then
    BOARDS=("$@")
    FULL_BUILD=0
else
    BOARDS=(devkitc guition-p4)
    FULL_BUILD=1
fi

mkdir -p "$OUTPUT_DIR"

# Full builds start from a clean slate so a previous partial run can't
# leave a stale .bin or firmware.json behind. Subset builds preserve
# the existing manifest because regenerating it from a subset would
# silently drop boards that weren't passed on the command line.
if [[ "$FULL_BUILD" -eq 1 ]]; then
    rm -f "$OUTPUT_DIR"/zenclaw-*.bin "$OUTPUT_DIR/firmware.json"
fi

# Manifest entries written as we go.
declare -a MANIFEST_ENTRIES

read_manifest_field() {
    local board="$1" field="$2"
    awk -F' *= *' -v f="^$field\$" '$1 ~ f {gsub(/"/,"",$2); print $2; exit}' "boards/$board.toml"
}

# Per-board human-friendly metadata that lives only in this script.
# Keep this in sync with boards/<id>.toml descriptions.
board_display_name() {
    case "$1" in
        devkitc)    echo "ESP32-S3 DevKitC" ;;
        guition-p4) echo "Guition JC-ESP32P4-M3-DEV" ;;
        *)          echo "$1" ;;
    esac
}

board_chip_label() {
    case "$1" in
        devkitc)    echo "ESP32-S3" ;;
        guition-p4) echo "ESP32-P4" ;;
        *)          echo "ESP32" ;;
    esac
}

board_network() {
    case "$1" in
        guition-p4) echo "ethernet" ;;
        *)          echo "wifi" ;;
    esac
}

board_default() {
    [[ "$1" == "devkitc" ]] && echo true || echo false
}

board_description() {
    case "$1" in
        devkitc)    echo "8MB PSRAM, USB Host capable" ;;
        guition-p4) echo "32MB PSRAM, Ethernet via IP101 PHY" ;;
        *)          echo "" ;;
    esac
}

for board in "${BOARDS[@]}"; do
    [[ -f "boards/$board.toml" ]] || { echo "unknown board: $board" >&2; exit 1; }

    chip=$(read_manifest_field "$board" chip)
    target=$(read_manifest_field "$board" target)
    bootloader=$(read_manifest_field "$board" bootloader)

    elf="target/$target/release/zenclaw-agent"
    out="$OUTPUT_DIR/zenclaw-$board.bin"

    echo "==> Building $board ($chip / $target)"
    just build "$board" --release

    echo "==> Saving merged image -> $out"
    espflash save-image \
        --chip "$chip" \
        --flash-size 16mb \
        --partition-table partitions.csv \
        --bootloader "$bootloader" \
        --merge \
        --skip-padding \
        "$elf" "$out"

    size=$(wc -c < "$out" | tr -d ' ')
    echo "    $(basename "$out"): ${size} bytes"

    MANIFEST_ENTRIES+=("$(cat <<JSON
    {
      "id": "$board",
      "name": "$(board_display_name "$board")",
      "chip": "$(board_chip_label "$board")",
      "image": "zenclaw-$board.bin",
      "network": "$(board_network "$board")",
      "default": $(board_default "$board"),
      "description": "$(board_description "$board")"
    }
JSON
)")
done

if [[ "$FULL_BUILD" -eq 1 ]]; then
    # Join manifest entries with commas
    joined=""
    for entry in "${MANIFEST_ENTRIES[@]}"; do
        if [[ -z "$joined" ]]; then
            joined="$entry"
        else
            joined="$joined,
$entry"
        fi
    done

    cat > "$OUTPUT_DIR/firmware.json" <<JSON
{
  "boards": [
$joined
  ]
}
JSON

    echo "==> Wrote $OUTPUT_DIR/firmware.json"
else
    echo "==> Subset build: $OUTPUT_DIR/firmware.json preserved (run without args to regenerate)"
fi

echo
echo "Done. Outputs:"
ls -lh "$OUTPUT_DIR"/zenclaw-*.bin "$OUTPUT_DIR"/firmware.json 2>/dev/null || true
