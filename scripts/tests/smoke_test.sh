#!/bin/bash
# BRP smoke test — starts the game with the `remote` feature, waits for the
# BRP server to become reachable, then exercises every custom JSON-RPC method.
#
# Usage:
#   ./scripts/tests/smoke_test.sh            # normal run
#   timeout 120 ./scripts/tests/smoke_test.sh  # with a hard deadline (CI)
set -euo pipefail

BRP_URL="http://localhost:15702"
GAME_PID=""
PASS=0
FAIL=0

cleanup() {
    if [ -n "$GAME_PID" ]; then
        kill "$GAME_PID" 2>/dev/null || true
        wait "$GAME_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Start the game ──────────────────────────────────────────────────────────

echo "Building and launching game with --features remote ..."
cargo run -p macrocosmo --features remote &
GAME_PID=$!

echo "Waiting for BRP server on $BRP_URL (PID $GAME_PID) ..."
for i in $(seq 1 60); do
    if curl -sf "$BRP_URL" >/dev/null 2>&1; then
        echo "BRP server ready after ${i}s"
        break
    fi
    if ! kill -0 "$GAME_PID" 2>/dev/null; then
        echo "FATAL: game process exited before BRP became available"
        exit 1
    fi
    sleep 1
done

# Verify the server is actually responding to requests.
if ! curl -sf "$BRP_URL" >/dev/null 2>&1; then
    echo "FATAL: BRP server did not become ready within 60 s"
    exit 1
fi

# ── Helpers ─────────────────────────────────────────────────────────────────

brp() {
    curl -sf -X POST "$BRP_URL" \
        -H "Content-Type: application/json" \
        -d "$1"
}

assert_ok() {
    local label="$1"
    local body="$2"
    local result
    result=$(brp "$body") || { echo "FAIL [$label]: curl error"; FAIL=$((FAIL+1)); return; }
    if echo "$result" | grep -q '"error"'; then
        echo "FAIL [$label]: $result"
        FAIL=$((FAIL+1))
    else
        echo "PASS [$label]"
        PASS=$((PASS+1))
    fi
}

assert_field() {
    local label="$1"
    local body="$2"
    local field="$3"
    local result
    result=$(brp "$body") || { echo "FAIL [$label]: curl error"; FAIL=$((FAIL+1)); return; }
    if echo "$result" | grep -q "\"$field\""; then
        echo "PASS [$label]"
        PASS=$((PASS+1))
    else
        echo "FAIL [$label]: missing field '$field' in $result"
        FAIL=$((FAIL+1))
    fi
}

# ── Tests ───────────────────────────────────────────────────────────────────

echo ""
echo "=== BRP Smoke Tests ==="
echo ""

# 1. bevy/list — list registered methods
echo "--- Test 1: bevy/list ---"
assert_ok "bevy/list" \
    '{"jsonrpc":"2.0","id":1,"method":"bevy/list","params":{}}'

# 2. bevy/query — query Ship entities
echo "--- Test 2: query Ship entities ---"
assert_ok "query ships" \
    '{"jsonrpc":"2.0","id":2,"method":"bevy/query","params":{"data":{"components":["macrocosmo::ship::Ship"]}}}'

# 3. macrocosmo/advance_time
echo "--- Test 3: advance_time ---"
assert_field "advance_time" \
    '{"jsonrpc":"2.0","id":3,"method":"macrocosmo/advance_time","params":{"hexadies":5}}' \
    "elapsed"

# 4. macrocosmo/eval_lua
echo "--- Test 4: eval_lua ---"
assert_field "eval_lua" \
    '{"jsonrpc":"2.0","id":4,"method":"macrocosmo/eval_lua","params":{"code":"return 1 + 1"}}' \
    "result"

# 5. macrocosmo/key_press (F3)
echo "--- Test 5: key_press ---"
assert_field "key_press F3" \
    '{"jsonrpc":"2.0","id":5,"method":"macrocosmo/key_press","params":{"key":"F3"}}' \
    "status"

# 6. macrocosmo/hover
echo "--- Test 6: hover ---"
assert_field "hover" \
    '{"jsonrpc":"2.0","id":6,"method":"macrocosmo/hover","params":{"x":400,"y":300}}' \
    "status"

# 7. macrocosmo/click
echo "--- Test 7: click ---"
assert_field "click" \
    '{"jsonrpc":"2.0","id":7,"method":"macrocosmo/click","params":{"x":400,"y":300}}' \
    "status"

# 8. macrocosmo/screenshot — first call triggers capture, second retrieves
echo "--- Test 8: screenshot ---"
# First request triggers capture (expected to return an error asking to retry).
brp '{"jsonrpc":"2.0","id":8,"method":"macrocosmo/screenshot","params":{}}' >/dev/null 2>&1 || true
sleep 1
# Second request should have the buffered result.
assert_field "screenshot" \
    '{"jsonrpc":"2.0","id":9,"method":"macrocosmo/screenshot","params":{}}' \
    "base64"

# ── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
