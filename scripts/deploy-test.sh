#!/usr/bin/env bash
# Deploy firmware to ESP32 and run smoke tests.
# Usage: ./scripts/deploy-test.sh [--files-only agent/runner.py lib/httpclient.py ...]
#
# With no args: full deploy (all firmware dirs + root files)
# With --files-only: deploy only the listed files (paths relative to firmware/)

set -euo pipefail
cd "$(dirname "$0")/.."

DEVICE_IP="${ZENCLAW_IP:-192.168.50.93}"
API_BASE="http://${DEVICE_IP}"
BOOT_WAIT=12

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

die() { red "FAIL: $*" >&2; exit 1; }

# ── Deploy ──────────────────────────────────────────────
if [[ "${1:-}" == "--files-only" ]]; then
    shift
    echo "Deploying individual files..."
    for f in "$@"; do
        # Map firmware-relative path to device path
        dest=":${f}"
        echo "  ${f} -> ${dest}"
        mpremote cp "firmware/${f}" "${dest}" || die "Failed to copy ${f}"
    done
else
    echo "Full deploy..."
    mpremote cp -r firmware/agent/ :agent/     || die "Failed to copy agent/"
    mpremote cp -r firmware/lib/ :lib/         || die "Failed to copy lib/"
    mpremote cp -r firmware/stubs/ :stubs/     || die "Failed to copy stubs/"
    mpremote cp firmware/boot.py firmware/main.py firmware/config.json \
        firmware/zenclaw_paths.py firmware/firmware-version.json : \
        || die "Failed to copy root files"
fi

# ── Reset ───────────────────────────────────────────────
echo "Resetting device..."
mpremote reset 2>/dev/null || true
echo "Waiting ${BOOT_WAIT}s for boot..."
sleep "$BOOT_WAIT"

# ── Test: API status ────────────────────────────────────
echo ""
echo "=== Smoke Tests ==="

status=$(curl -sf --max-time 10 "${API_BASE}/api/status" 2>&1) || die "API /api/status unreachable"
green "PASS: /api/status reachable"
echo "  ${status}" | python3 -m json.tool 2>/dev/null || echo "  ${status}"

# ── Test: agent name present ────────────────────────────
echo "$status" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('agent_name'), 'missing agent_name'" \
    && green "PASS: agent_name present" \
    || die "agent_name missing from status"

# ── Test: WiFi connected ────────────────────────────────
echo "$status" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('wifi',{}).get('connected'), 'wifi not connected'" \
    && green "PASS: WiFi connected" \
    || yellow "WARN: WiFi not connected"

# ── Test: Chat endpoint exists ──────────────────────────
chat_resp=$(curl -sf --max-time 5 -X POST "${API_BASE}/api/chat" \
    -H 'Content-Type: application/json' \
    -d '{"message":"ping"}' 2>&1) \
    && green "PASS: /api/chat responds" \
    || yellow "WARN: /api/chat failed (may need LLM key)"
if [[ -n "${chat_resp:-}" ]]; then
    echo "  ${chat_resp}"
fi

# ── Test: Chat history endpoint ─────────────────────────
hist_resp=$(curl -sf --max-time 5 "${API_BASE}/api/chat/history?chat_id=web&limit=5" 2>&1) \
    && green "PASS: /api/chat/history responds" \
    || yellow "WARN: /api/chat/history failed"

# ── Test: Files endpoint ────────────────────────────────
files_resp=$(curl -sf --max-time 5 "${API_BASE}/api/files?path=/" 2>&1) \
    && green "PASS: /api/files responds" \
    || yellow "WARN: /api/files failed"

# ── Test: Config endpoint ───────────────────────────────
config_resp=$(curl -sf --max-time 5 "${API_BASE}/api/config" 2>&1) \
    && green "PASS: /api/config responds" \
    || yellow "WARN: /api/config failed"

echo ""
green "=== Deploy + test complete ==="
