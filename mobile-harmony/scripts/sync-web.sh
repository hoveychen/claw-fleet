#!/usr/bin/env bash
# Sync mobile-web build artifacts into rawfile for offline loading by WebShell.
#
# Usage:
#   bash scripts/sync-web.sh                    # Build production relay
#   RELAY_URL=http://127.0.0.1:18080 bash scripts/sync-web.sh   # Build local relay
#
# Both parameters are mandatory, both proven on real hardware — do not change:
#
#   --base=./  Vite defaults to outputting `/assets/…` with absolute paths, which
#              do not resolve when loaded by WebShell, resulting in a blank screen
#              with no error. Capacitor is unaffected because it serves the entire
#              directory via http://localhost.
#
#   VITE_RELAY_URL  WebShell loads pages from rawfile with origin https://fleet.local,
#              and relayHttpBase() without this variable falls back to origin — that
#              would make the app connect to itself. The real relay must be baked in
#              at compile time.
set -e
cd "$(dirname "$0")/.."

RELAY_URL="${RELAY_URL:-https://fleet-relay.muveeai.com}"
WEB_DIR="../mobile-web"
DEST="entry/src/main/resources/rawfile/web"

if [[ ! -d "$WEB_DIR" ]]; then
  echo "✗ 找不到 $WEB_DIR —— 本脚本假定鸿蒙工程与 mobile-web 同仓" >&2
  exit 1
fi

echo "→ 构建 mobile-web (relay: $RELAY_URL)"
( cd "$WEB_DIR" && VITE_RELAY_URL="$RELAY_URL" pnpm exec vite build --base=./ >/dev/null )

echo "→ 同步进 $DEST"
rm -rf "$DEST"
mkdir -p "$DEST"
cp -R "$WEB_DIR/dist/." "$DEST/"

# The two most common causes of blank screens are missing these two things; catching them at build time is much cheaper than debugging on real hardware.
[[ -f "$DEST/index.html" ]] || { echo "✗ 同步后没有 index.html" >&2; exit 1; }
grep -q 'src="\./' "$DEST/index.html" || {
  echo "✗ index.html 里不是相对路径 —— --base=./ 没生效，装上会白屏" >&2; exit 1; }

echo "✓ web 已同步 ($(du -sh "$DEST" | cut -f1))"
