#!/bin/bash
# End-to-end tool tests via /api/chat on the ESP32 device.
# Usage: ./test_tools_e2e.sh [host]
# Each test sends a chat message that forces a specific tool call, then checks the result.

HOST="${1:-zenclaw.local}"
BASE="http://$HOST"
PASS=0
FAIL=0
CHAT_ID="e2etest_$$"

chat() {
  curl -sf --max-time 120 "$BASE/api/chat" \
    -H 'Content-Type: application/json' \
    -d "{\"message\":\"$1\",\"chat_id\":\"$CHAT_ID\"}" 2>/dev/null
}

read_file() {
  curl -sf --max-time 10 "$BASE/api/files/read?path=$1" 2>/dev/null
}

check() {
  local name="$1" reply="$2" pattern="$3"
  if echo "$reply" | grep -qi "$pattern"; then
    echo "  PASS: $name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $name (expected '$pattern')"
    echo "    got: $(echo "$reply" | head -c 200)"
    FAIL=$((FAIL + 1))
  fi
}

echo "=== ZenClaw Tool E2E Tests ==="
echo "Host: $BASE"
echo "Chat ID: $CHAT_ID"
echo ""

# --- memory save ---
echo "[1/8] memory save"
R=$(chat 'Use the memory tool with action="save" and content="E2E test memory entry XYZ123". Do not say anything else, just call the tool.')
check "memory save" "$R" "saved\|memory\|XYZ123"

# Verify file was created
echo "  Verifying /data/MEMORY.md..."
MEM=$(read_file "/data/MEMORY.md")
if echo "$MEM" | grep -q "XYZ123"; then
  echo "  PASS: MEMORY.md contains entry"
  PASS=$((PASS + 1))
else
  echo "  FAIL: MEMORY.md missing entry"
  echo "    content: $(echo "$MEM" | head -c 200)"
  FAIL=$((FAIL + 1))
fi

# --- memory search ---
echo "[2/8] memory search"
R=$(chat 'Use the memory tool with action="search" and content="XYZ123". Just call the tool and show the result.')
check "memory search" "$R" "XYZ123\|test memory"

# --- file write ---
echo "[3/8] file write"
R=$(chat 'Use the file tool with action="write", path="e2e_test.txt", content="hello from e2e test". Just call the tool.')
check "file write" "$R" "wrote\|written\|success\|e2e_test"

# Verify
echo "  Verifying file..."
FC=$(read_file "/data/e2e_test.txt")
if echo "$FC" | grep -q "hello from e2e test"; then
  echo "  PASS: file content correct"
  PASS=$((PASS + 1))
else
  echo "  FAIL: file content wrong"
  echo "    content: $(echo "$FC" | head -c 200)"
  FAIL=$((FAIL + 1))
fi

# --- file read ---
echo "[4/8] file read"
R=$(chat 'Use the file tool with action="read", path="e2e_test.txt". Show me the content.')
check "file read" "$R" "hello from e2e test"

# --- file edit ---
echo "[5/8] file edit"
R=$(chat 'Use the file tool with action="edit", path="e2e_test.txt", old_string="hello", new_string="goodbye". Just call the tool.')
check "file edit" "$R" "edited\|replaced\|success\|goodbye"

# Verify
FC=$(read_file "/data/e2e_test.txt")
if echo "$FC" | grep -q "goodbye from e2e test"; then
  echo "  PASS: edit applied"
  PASS=$((PASS + 1))
else
  echo "  FAIL: edit not applied"
  echo "    content: $(echo "$FC" | head -c 200)"
  FAIL=$((FAIL + 1))
fi

# --- file list_dir ---
echo "[6/8] file list_dir"
R=$(chat 'Use the file tool with action="list_dir", path=".". List the root directory.')
check "file list_dir" "$R" "SOUL\|sessions\|e2e_test"

# --- file delete ---
echo "[7/8] file delete"
R=$(chat 'Use the file tool with action="delete", path="e2e_test.txt". Just call the tool.')
check "file delete" "$R" "deleted\|removed\|success"

# --- session status ---
echo "[8/8] session status"
R=$(chat 'Use the session tool with action="status". Show the result.')
check "session status" "$R" "chat_id\|platform\|agent"

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
exit $FAIL
