#!/usr/bin/env bash
# Build the web bundle for the native (Capacitor) shell and sync it into the
# android/ios projects.
#
# Why this exists instead of plain `pnpm build`: the PWA is served BY the relay,
# so `relayHttpBase()` resolving to `window.location.origin` is correct there.
# Inside the shell the origin is `http://localhost` / `capacitor://localhost`,
# so the same fallback would point the app at itself and every request would
# fail with no obvious cause. A shell build must therefore bake in the relay
# URL at compile time.
#
# Override the target relay with RELAY_URL=... (e.g. a local relay for testing).
set -euo pipefail

cd "$(dirname "$0")/.."

# Matches claw-fleet-core/src/mobile_relay.rs DEFAULT_RELAY_URL, i.e. the
# non-mainland host. A build can't know where the phone will be, so mainland
# builds pass RELAY_URL=https://fleet-relay.eternizedlab.com explicitly
# (both hosts front the same relay — see claw-fleet-core/src/relay_region.rs).
RELAY_URL="${RELAY_URL:-https://fleet-relay.muveeai.com}"

echo "==> building shell bundle against relay: $RELAY_URL"
VITE_RELAY_URL="$RELAY_URL" pnpm build

echo "==> syncing native projects"
pnpm exec cap sync

echo "==> done"
