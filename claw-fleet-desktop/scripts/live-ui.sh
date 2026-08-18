#!/usr/bin/env bash
# Start the browser harness that runs the REAL desktop frontend against REAL
# backend data: `fleet serve` (probe) + vite dev + `?mock&live`.
#
#   scripts/live-ui.sh            # start, print the URL, keep running
#   FLEET_BIN=... UI_PORT=5199 PROBE_PORT=8791 scripts/live-ui.sh
#
# Stop with Ctrl-C (both children are killed). Logs land in
# /tmp/fleet-live-ui/{probe,vite}.log so a headless driver can tail them.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLEET_BIN="${FLEET_BIN:-$HOME/.fleet/bin/fleet}"
UI_PORT="${UI_PORT:-5199}"
PROBE_PORT="${PROBE_PORT:-8791}"
TOKEN="${FLEET_LIVE_TOKEN:-live-harness-token}"
LOGDIR=/tmp/fleet-live-ui
mkdir -p "$LOGDIR"

[ -x "$FLEET_BIN" ] || { echo "no fleet binary at $FLEET_BIN (set FLEET_BIN)"; exit 1; }

# dsh sessions are scanned through the dsh CLI/web API; make sure the probe can
# find it the same way the desktop app does.
if [ -z "${FLEET_DSH_BIN:-}" ] && command -v dsh >/dev/null 2>&1; then
  export FLEET_DSH_BIN="$(command -v dsh)"
fi

echo "→ probe:  $FLEET_BIN serve --port $PROBE_PORT  (log: $LOGDIR/probe.log)"
"$FLEET_BIN" serve --port "$PROBE_PORT" --token "$TOKEN" >"$LOGDIR/probe.log" 2>&1 &
PROBE_PID=$!

cleanup() { kill "$PROBE_PID" "${VITE_PID:-}" 2>/dev/null; }
trap cleanup EXIT INT TERM

for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:$PROBE_PORT/health?token=$TOKEN" >/dev/null && break
  sleep 0.25
done
curl -sf "http://127.0.0.1:$PROBE_PORT/health?token=$TOKEN" >/dev/null \
  || { echo "probe never became healthy — see $LOGDIR/probe.log"; exit 1; }
echo "  probe healthy (pid $PROBE_PID)"

export FLEET_LIVE_PROBE="http://127.0.0.1:$PROBE_PORT"
export FLEET_LIVE_TOKEN="$TOKEN"
echo "→ vite:   port $UI_PORT  (log: $LOGDIR/vite.log)"
(cd "$HERE" && pnpm exec vite --port "$UI_PORT" --strictPort >"$LOGDIR/vite.log" 2>&1) &
VITE_PID=$!

for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$UI_PORT/" >/dev/null && break
  sleep 0.5
done
echo
echo "  open  http://127.0.0.1:$UI_PORT/?mock&live"
echo "  (?mock installs the Tauri shims; ?live routes data commands to the probe)"
echo
wait
