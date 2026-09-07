#!/usr/bin/env bash
# Build a local *development* version of Claw Fleet and install it to /Applications.
#
# This is debug-profile only so cargo can use incremental compilation across
# runs. Cargo.toml is NOT mutated — the in-app version reads 0.0.0 from the
# committed manifest. DMG/PKG filenames carry a date stamp so successive builds
# don't clobber each other.
#
# Real releases happen via GitHub CI on tag push (see .github/workflows/{ci,release}.yml).
# This script is intentionally not a release tool.
#
# Usage: ./scripts/build-local.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

# Tauri sets MACOSX_DEPLOYMENT_TARGET from bundle.macOS.minimumSystemVersion
# while compiling the desktop app. The CLI sidecar is compiled first into the
# same Cargo target directory, so leaving the variable unset here makes Cargo
# flip the native dependency tree between "unset" and the Tauri value on every
# run. Read the single source of truth up front so both builds use one cache.
if [[ "$(uname)" == "Darwin" ]]; then
  MACOSX_DEPLOYMENT_TARGET="$(python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1]))["bundle"]["macOS"]["minimumSystemVersion"])' \
    "$ROOT_DIR/claw-fleet-desktop/tauri.conf.json")"
  if [[ -z "$MACOSX_DEPLOYMENT_TARGET" ]]; then
    echo "==> Missing bundle.macOS.minimumSystemVersion in tauri.conf.json" >&2
    exit 1
  fi
  export MACOSX_DEPLOYMENT_TARGET
  echo "==> macOS deployment target: $MACOSX_DEPLOYMENT_TARGET"
fi

# ── Serialise concurrent builds ──────────────────────────────────────────────
# One build runs ~10 parallel rustc (measured peak: 1224MB for the largest
# single rustc, 2.9GB for all of them together) on top of vite/esbuild and the
# tauri bundler. Two overlapping builds are what turn a busy machine into a
# swapping one.
#
# The lock is machine-global on purpose, not under target/: every worktree has
# its own target/, so a repo-local lock would not see a build running in a
# sibling worktree — which is exactly the overlap that happens in practice.
#
# It is also NOT under $TMPDIR, for the same reason the cargo gate's slot store
# is not (see scripts/cargo-jobs-guard.sh): TMPDIR is a per-user launchd
# directory on macOS, and a Fleet session spawned detached need not inherit it.
# Such a session falls back to /tmp, takes a *different* lock, and the two
# builds overlap anyway — silently, because each one believes it holds the
# global lock.
#
# This lock is coarser than, and complementary to, the cargo gate: the gate
# bounds concurrent rustc, while this bounds concurrent *builds*, which also
# contend over pnpm, the tauri bundler and claw-fleet-desktop/binaries/.
#
# `mkdir` is the atomic primitive here because macOS has no `flock(1)`.
BUILD_LOCK="/tmp/claw-fleet-build-local-$(id -u).lock"
if ! mkdir "$BUILD_LOCK" 2>/dev/null; then
  holder="$(cat "$BUILD_LOCK/owner" 2>/dev/null || true)"
  holder_pid="${holder%% *}"
  if [[ -n "$holder_pid" ]] && kill -0 "$holder_pid" 2>/dev/null; then
    echo "==> Another build-local.sh is already running:" >&2
    echo "    $holder" >&2
    echo "    Wait for it, or kill $holder_pid, then re-run." >&2
    exit 1
  fi
  # Holder is gone (Ctrl-C, crash, reboot) — the lock is stale, not held.
  # Without this a single interrupted build would wedge every later one.
  echo "==> Clearing a stale build lock left by: ${holder:-unknown}" >&2
  rm -rf "$BUILD_LOCK"
  mkdir "$BUILD_LOCK" || { echo "==> could not acquire build lock" >&2; exit 1; }
fi
trap 'rm -rf "$BUILD_LOCK"' EXIT
echo "$$ $ROOT_DIR $(date '+%Y-%m-%d %H:%M:%S')" > "$BUILD_LOCK/owner"

# ── Cap cargo's parallelism against *available* memory ───────────────────────
# Not against core count: on a machine already carrying a day's worth of agent
# sessions, 10 concurrent rustc is fine when there is room and ruinous when
# there isn't. The 2GB divisor comes from the measured 1224MB peak of a single
# rustc in this workspace, rounded up for headroom. Only lowers the job count,
# never raises it, and FLEET_BUILD_JOBS overrides the whole calculation.
#
# Kept here even though the cargo shim computes the same cap: the shim is an
# opt-in install (scripts/install-cargo-guard.sh), so a machine without it
# would otherwise lose this protection entirely. When the shim IS installed it
# inherits the CARGO_BUILD_JOBS exported below rather than recomputing.
if [[ -z "${FLEET_BUILD_JOBS:-}" ]] && command -v vm_stat >/dev/null 2>&1; then
  _page=$(vm_stat | sed -n '1s/.*page size of \([0-9]*\).*/\1/p')
  _free=$(vm_stat | awk '/Pages (free|inactive|speculative)/ {gsub(/\./,"",$NF); s+=$NF} END {print s+0}')
  if [[ -n "$_page" && "$_free" -gt 0 ]]; then
    _avail_gb=$(( _free * _page / 1073741824 ))
    _ncpu=$(sysctl -n hw.ncpu)
    _jobs=$(( _avail_gb / 2 ))
    (( _jobs < 2 )) && _jobs=2
    if (( _jobs < _ncpu )); then
      export CARGO_BUILD_JOBS="$_jobs"
      echo "==> Only ${_avail_gb}GB available — capping cargo at $_jobs jobs (of $_ncpu cores)"
    fi
  fi
fi
if [[ -n "${FLEET_BUILD_JOBS:-}" ]]; then
  export CARGO_BUILD_JOBS="$FLEET_BUILD_JOBS"
  echo "==> FLEET_BUILD_JOBS set — cargo capped at $CARGO_BUILD_JOBS jobs"
fi

# ── Load signing config (optional) ───────────────────────────────────────────
SIGNING_CONF="$SCRIPT_DIR/signing.local"
APPLE_SIGNING_IDENTITY=""
APPLE_INSTALLER_IDENTITY=""
if [[ -f "$SIGNING_CONF" ]]; then
  # signing.local is a user-local, gitignored file — there is no in-repo target
  # for shellcheck to follow, so silence SC1090 rather than chase a phantom path.
  # shellcheck source=/dev/null
  source "$SIGNING_CONF"
  echo "==> Signing identity: $APPLE_SIGNING_IDENTITY"
  [[ -n "$APPLE_INSTALLER_IDENTITY" ]] && echo "==> Installer identity: $APPLE_INSTALLER_IDENTITY"
else
  echo "==> No scripts/signing.local — using ad-hoc signing"
fi

# Human-readable stamp for DMG/PKG filenames so you can tell builds apart.
# NOT sed'd into Cargo.toml — that would invalidate cargo's fingerprint cache
# on every run and force a full recompile.
BUILD_STAMP="$(date +%y%m%d-%H%M%S)"
echo "==> Build stamp: $BUILD_STAMP (debug)"

export OPENSSL_STATIC=1

# Detect native target triple
TARGET=$(rustc -vV | sed -n 's|host: ||p')
echo "==> Target: $TARGET"

# Where cargo actually writes artifacts. Deliberately NOT hardcoded to
# `target/`: callers may redirect it with CARGO_TARGET_DIR,
# CARGO_BUILD_TARGET_DIR, or a local Cargo config. Every path below (sidecar
# staging, app bundle, DMG, PKG) has to follow the resolved directory.
#
# Getting this wrong fails *silently*: the app-bundle check below prints
# "skipping macOS post-steps" and exits 0, so a stale bundle looks like a
# successful build. `cargo metadata` is the only authority — it accounts for
# CARGO_TARGET_DIR, CARGO_BUILD_TARGET_DIR and build.target-dir alike. It is
# also exempt from the cargo concurrency guard, so it never queues.
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null || true)"
if [[ -z "$TARGET_DIR" || ! -d "$TARGET_DIR" ]]; then
  echo "==> Could not resolve cargo target_directory; falling back to ./target" >&2
  TARGET_DIR="target"
fi
echo "==> Target dir: $TARGET_DIR"

# 1. Build fleet CLI sidecar (debug)
echo "==> Building fleet CLI..."
cargo build -p fleet-cli

# 2. Copy compiled binaries into binaries/ so Tauri bundles the real binary.
#    Only update if content differs — Tauri's externalBin watcher invalidates
#    the desktop crate's build script on any mtime change in binaries/,
#    which forces a recompile even when nothing actually changed.
mkdir -p claw-fleet-desktop/binaries

# stage_sidecar <target-dir-binary-name> <externalBin-base-name>
# Copies $TARGET_DIR/debug/<bin> → binaries/<base>-$TARGET only when content
# differs, and mirrors a generic <base>-linux name for deb.files on Linux.
stage_sidecar() {
  local bin="$1" base="$2"
  local src="$TARGET_DIR/debug/$bin"
  local dst="claw-fleet-desktop/binaries/$base-$TARGET"
  if [[ ! -f "$dst" ]] || ! cmp -s "$src" "$dst"; then
    cp "$src" "$dst"
    chmod +x "$dst"
    echo "==> Copied $bin → $dst"
  else
    echo "==> $bin sidecar unchanged at $dst (skip copy)"
  fi
  if [[ "$(uname)" == "Linux" ]]; then
    local linux_dst="claw-fleet-desktop/binaries/$base-linux"
    if [[ ! -f "$linux_dst" ]] || ! cmp -s "$dst" "$linux_dst"; then
      cp "$dst" "$linux_dst"
      chmod +x "$linux_dst"
    fi
  fi
}

stage_sidecar fleet-cli fleet

# 3. Install frontend deps (pnpm, matching CI). tauri's beforeBuildCommand runs
#    `pnpm build`, and `npx tauri` itself resolves @tauri-apps/cli from
#    node_modules — both need the workspace installed first.
echo "==> Installing frontend deps (pnpm)..."
(cd claw-fleet-desktop && pnpm install --frozen-lockfile)

# 4. Build Tauri app (debug)
echo "==> Building Tauri app..."
(cd claw-fleet-desktop && npx tauri build --debug --bundles app)

# 5. Sign with entitlements (macOS only)
#    Sign the sidecar FIRST with its own (non-sandbox) entitlements, then the
#    outer app bundle. Using --deep would overwrite the sidecar signature with
#    the app's sandbox entitlements, causing SIGTRAP when the sidecar is invoked
#    externally (e.g. fleet by Claude Code hooks). Kept as a loop so adding
#    sidecars later stays a one-line change.
APP_BUNDLE="$TARGET_DIR/debug/bundle/macos/Claw Fleet.app"
SIDECARS=("$APP_BUNDLE/Contents/MacOS/fleet")
if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "==> App bundle not produced at $APP_BUNDLE — skipping macOS post-steps."
  echo "Done! Build stamp: $BUILD_STAMP"
  exit 0
fi

if [[ -n "$APPLE_SIGNING_IDENTITY" ]]; then
  for sc in "${SIDECARS[@]}"; do
    echo "==> Signing sidecar $(basename "$sc") with non-sandbox entitlements..."
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
      --entitlements claw-fleet-desktop/entitlements-sidecar.plist \
      --options runtime \
      "$sc"
  done
  echo "==> Signing app bundle with sandbox entitlements..."
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
    --entitlements claw-fleet-desktop/entitlements.plist \
    --options runtime \
    "$APP_BUNDLE"
else
  for sc in "${SIDECARS[@]}"; do
    echo "==> Ad-hoc signing sidecar $(basename "$sc") with non-sandbox entitlements..."
    codesign --force --sign - \
      --entitlements claw-fleet-desktop/entitlements-sidecar.plist \
      "$sc"
  done
  echo "==> Ad-hoc signing app bundle with entitlements..."
  codesign --force --sign - \
    --entitlements claw-fleet-desktop/entitlements.plist \
    "$APP_BUNDLE"
fi

# 6. Create DMG
DMG_DIR="$TARGET_DIR/debug/bundle/dmg"
mkdir -p "$DMG_DIR"
DMG_NAME="claw-fleet-dev-${BUILD_STAMP}.dmg"
echo "==> Creating DMG..."
DMG_STAGING=$(mktemp -d)
cp -R "$APP_BUNDLE" "$DMG_STAGING/"
ln -s /Applications "$DMG_STAGING/Applications"
hdiutil create -volname "Claw Fleet" \
  -srcfolder "$DMG_STAGING" \
  -ov -format UDZO \
  "$DMG_DIR/$DMG_NAME"
rm -rf "$DMG_STAGING"
echo "==> DMG: $DMG_DIR/$DMG_NAME"

# 7. Build PKG installer
PKG_DIR="$TARGET_DIR/debug/bundle/pkg"
mkdir -p "$PKG_DIR"
PKG_NAME="claw-fleet-dev-${BUILD_STAMP}.pkg"
echo "==> Building PKG installer..."

# Disable two PackageKit defaults that break local dev installs:
#   - BundleIsRelocatable: macOS would otherwise redirect the install to
#     the first matching bundle registered in LaunchServices (e.g. the
#     app still sitting in target/debug/bundle/macos/), silently
#     ignoring --install-location.
#   - BundleIsVersionChecked: the 0.0.0 dev version is numerically lower
#     than any released 1.x.y, so without this PackageKit would skip the
#     component with "version X.Y.Z is already installed" and leave the
#     installed release bundle untouched.
PKG_STAGING=$(mktemp -d)
cp -R "$APP_BUNDLE" "$PKG_STAGING/"
COMPONENT_PLIST="$PKG_STAGING/component.plist"
pkgbuild --analyze --root "$PKG_STAGING" "$COMPONENT_PLIST"
/usr/libexec/PlistBuddy -c "Set :0:BundleIsRelocatable false" "$COMPONENT_PLIST"
/usr/libexec/PlistBuddy -c "Set :0:BundleIsVersionChecked false" "$COMPONENT_PLIST"

PKG_VERSION="0.0.0-dev.${BUILD_STAMP}"
if [[ -n "$APPLE_INSTALLER_IDENTITY" ]]; then
  pkgbuild --root "$PKG_STAGING" \
    --component-plist "$COMPONENT_PLIST" \
    --identifier "com.hoveychen.claw-fleet" \
    --version "$PKG_VERSION" \
    --install-location "/Applications" \
    --sign "$APPLE_INSTALLER_IDENTITY" \
    "$PKG_DIR/$PKG_NAME"
else
  pkgbuild --root "$PKG_STAGING" \
    --component-plist "$COMPONENT_PLIST" \
    --identifier "com.hoveychen.claw-fleet" \
    --version "$PKG_VERSION" \
    --install-location "/Applications" \
    "$PKG_DIR/$PKG_NAME"
fi
rm -rf "$PKG_STAGING"
echo "==> PKG: $PKG_DIR/$PKG_NAME"
open "$PKG_DIR/$PKG_NAME"

echo ""
echo "Done! Build stamp: $BUILD_STAMP"
echo "App bundle: $TARGET_DIR/debug/bundle/"
