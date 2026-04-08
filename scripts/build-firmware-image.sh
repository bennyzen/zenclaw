#!/bin/bash
# Build the ZenClaw filesystem image for flashing to ESP32-S3.
# Requires: littlefs-python (pipx install littlefs-python)
#
# Usage: ./scripts/build-firmware-image.sh
# Output: web/public/firmware/zenclaw.img

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
FIRMWARE_DIR="$REPO_ROOT/firmware"
OUTPUT_DIR="$REPO_ROOT/web/public/firmware"
STAGING_DIR="$REPO_ROOT/.firmware-staging"

# ESP32-S3 partition table (official MicroPython ESP32_GENERIC_S3):
#   factory  0x010000  0x1F0000 (2MB)
#   vfs      0x200000  auto-sized to remaining flash
# Detected from device: 16MB flash → VFS = 0xE00000 (14MB)
VFS_SIZE=0xE00000
BLOCK_SIZE=4096

echo "Building ZenClaw filesystem image..."

# Clean staging area
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"

# Copy firmware files, excluding junk and runtime data
rsync -a \
  --exclude='__pycache__' \
  --exclude='*.pyc' \
  --exclude='data/' \
  --exclude='.git' \
  "$FIRMWARE_DIR/" "$STAGING_DIR/"

# Include bootstrap files from data/
mkdir -p "$STAGING_DIR/data"
for f in SOUL.md AGENTS.md; do
  if [ -f "$FIRMWARE_DIR/data/$f" ]; then
    cp "$FIRMWARE_DIR/data/$f" "$STAGING_DIR/data/$f"
  fi
done

# Stamp build date into firmware-version.json
BUILD_TS="$(date -u +%Y%m%d%H%M%S)"
BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
python3 -c "
import json
with open('$STAGING_DIR/firmware-version.json', 'r') as f:
    v = json.load(f)
base = v.get('platform', '0.0.1').split('-')[0]
v['platform'] = base + '-' + '$BUILD_TS'
v['built'] = '$BUILD_DATE'
with open('$STAGING_DIR/firmware-version.json', 'w') as f:
    json.dump(v, f)
"

echo "  Staged files:"
find "$STAGING_DIR" -type f | wc -l
echo "  files (v${BUILD_TS})"

mkdir -p "$OUTPUT_DIR"

littlefs-python create \
  --block-size "$BLOCK_SIZE" \
  --fs-size "$VFS_SIZE" \
  "$STAGING_DIR" \
  "$OUTPUT_DIR/zenclaw.img"

# Cleanup
rm -rf "$STAGING_DIR"

echo "Done: $(du -h "$OUTPUT_DIR/zenclaw.img" | cut -f1) -> $OUTPUT_DIR/zenclaw.img"
