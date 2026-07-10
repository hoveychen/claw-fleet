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

# ── Load signing config (optional) ───────────────────────────────────────────
SIGNING_CONF="$SCRIPT_DIR/signing.local"
APPLE_SIGNING_IDENTITY=""
APPLE_INSTALLER_IDENTITY=""
if [[ -f "$SIGNING_CONF" ]]; then
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

# 1. Build fleet CLI sidecar (debug)
echo "==> Building fleet CLI..."
cargo build -p fleet-cli

# 2. Copy compiled binaries into binaries/ so Tauri bundles the real binary.
#    Only update if content differs — Tauri's externalBin watcher invalidates
#    the desktop crate's build script on any mtime change in binaries/,
#    which forces a recompile even when nothing actually changed.
mkdir -p claw-fleet-desktop/binaries

# stage_sidecar <target-dir-binary-name> <externalBin-base-name>
# Copies target/debug/<bin> → binaries/<base>-$TARGET only when content differs,
# and mirrors a generic <base>-linux name for deb.files on Linux.
stage_sidecar() {
  local bin="$1" base="$2"
  local src="target/debug/$bin"
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
APP_BUNDLE="target/debug/bundle/macos/Claw Fleet.app"
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
DMG_DIR="target/debug/bundle/dmg"
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
PKG_DIR="target/debug/bundle/pkg"
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
echo "App bundle: target/debug/bundle/"
