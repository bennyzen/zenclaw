#!/usr/bin/env bash
# Usage: eval "$(scripts/board-env.sh <board>)"
# Emits TARGET, SDKCONFIG, BOOTLOADER, FEATURES, BAUD env exports.
set -euo pipefail

board="${1:?usage: board-env.sh <board>}"
manifest="boards/$board.toml"
[[ -f "$manifest" ]] || { echo "echo 'no such manifest: $manifest' >&2; exit 1"; exit 1; }

# Tiny TOML reader (only handles strings + arrays of strings used in manifests)
# sep: separator to use between array elements (default ";")
get() {
    local sep="${2:-;}"
    awk -v key="$1" -v sep="$sep" '
        $0 ~ "^" key " *= *" {
            sub(/^[^=]*= */, "")
            gsub(/"/, "")
            gsub(/^\[ *| *\] *$/, "")
            gsub(/, +/, sep)
            print
            exit
        }
    ' "$manifest"
}

name=$(get name)
target=$(get target)
sdkconfig=$(get sdkconfig)
bootloader=$(get bootloader)
features=$(get features ,)
baud=$(get default_baud)

echo "export BOARD_NAME=\"${name:-$board}\""
echo "export TARGET=\"$target\""
echo "export SDKCONFIG=\"$sdkconfig\""
echo "export BOOTLOADER=\"$bootloader\""
echo "export FEATURES=\"$features\""
echo "export BAUD=\"${baud:-460800}\""
