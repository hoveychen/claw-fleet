#!/usr/bin/env bash
# Build Claw Fleet locally (GUI app + fleet CLI sidecar).
# Usage: ./scripts/build-local.sh [--debug] [--notarize]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

MODE="release"
CARGO_FLAG="--release"
NOTARIZE=false

for arg in "$@"; do
  case "$arg" in
    --debug)    MODE="debug"; CARGO_FLAG="" ;;
    --notarize) NOTARIZE=true ;;
  esac
done

# ── Load signing config ──────────────────────────────────────────────────────
SIGNING_CONF="$SCRIPT_DIR/signing.local"
APPLE_SIGNING_IDENTITY=""
APPLE_INSTALLER_IDENTITY=""
if [[ -f "$SIGNING_CONF" ]]; then
  source "$SIGNING_CONF"
  echo "==> Signing identity: $APPLE_SIGNING_IDENTITY"
  [[ -n "$APPLE_INSTALLER_IDENTITY" ]] && echo "==> Installer identity: $APPLE_INSTALLER_IDENTITY"
else
  echo "==> No scripts/signing.local found — will use ad-hoc signing (no sandbox, no notarization)"
fi

# ── Generate dev version ──────────────────────────────────────────────────────
# Keep the numeric major/minor at 0.0 so macOS pkg installers never treat this
# dev build as newer than an official SemVer release (e.g. 1.5.9). The date
# lives in the patch segment so the version is still human-readable.
YY=$(date +%y)
MM=$(date +%m)
DD=$(date +%d)
DEV_VERSION="0.0.${YY}${MM}${DD}-dev.$(date +%s)"
echo "==> Dev version: $DEV_VERSION"

export OPENSSL_STATIC=1

# Patch all Cargo.toml versions and restore on exit
for toml in claw-fleet-core/Cargo.toml claw-fleet-desktop/Cargo.toml fleet-cli/Cargo.toml; do
  cp "$toml" "${toml}.bak"
  sed -i.tmp "s/^version = \".*\"/version = \"${DEV_VERSION}\"/" "$toml"
  rm -f "${toml}.tmp"
done
trap 'for toml in claw-fleet-core/Cargo.toml claw-fleet-desktop/Cargo.toml fleet-cli/Cargo.toml; do mv "${toml}.bak" "$toml"; done' EXIT

# Detect native target triple
TARGET=$(rustc -vV | sed -n 's|host: ||p')
echo "==> Target: $TARGET  (mode: $MODE)"

# 1. Build fleet CLI sidecar
echo "==> Building fleet CLI (native)..."
cargo build $CARGO_FLAG -p fleet-cli

# 2. Copy compiled binary into binaries/ so Tauri bundles the real binary
mkdir -p claw-fleet-desktop/binaries
SRC="target/$MODE/fleet-cli"
DST="claw-fleet-desktop/binaries/fleet-$TARGET"
cp "$SRC" "$DST"
chmod +x "$DST"
echo "==> Copied fleet CLI → $DST"

# Linux: deb.files needs a generic fleet-linux name
if [[ "$(uname)" == "Linux" ]]; then
  cp "$DST" "claw-fleet-desktop/binaries/fleet-linux"
  chmod +x "claw-fleet-desktop/binaries/fleet-linux"
fi

# 3. Build Tauri app (run from claw-fleet-desktop/ where package.json lives)
echo "==> Building Tauri app..."
(cd claw-fleet-desktop && npx tauri build --bundles app)

# 4. Sign with entitlements (macOS only)
#    Sign the fleet CLI sidecar FIRST with its own (non-sandbox) entitlements,
#    then sign the outer app bundle.  Using --deep would overwrite the sidecar
#    signature with the app's sandbox entitlements, causing SIGTRAP when the
#    sidecar is invoked externally (e.g. by Claude Code hooks).
APP_BUNDLE="target/$MODE/bundle/macos/Claw Fleet.app"
SIDECAR="$APP_BUNDLE/Contents/MacOS/fleet"
if [[ -d "$APP_BUNDLE" ]]; then
  if [[ -n "$APPLE_SIGNING_IDENTITY" ]]; then
    echo "==> Signing sidecar (fleet CLI) with non-sandbox entitlements..."
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
      --entitlements claw-fleet-desktop/entitlements-sidecar.plist \
      --options runtime \
      "$SIDECAR"
    echo "==> Signing app bundle with sandbox entitlements..."
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
      --entitlements claw-fleet-desktop/entitlements.plist \
      --options runtime \
      "$APP_BUNDLE"
  else
    echo "==> Ad-hoc signing sidecar with non-sandbox entitlements..."
    codesign --force --sign - \
      --entitlements claw-fleet-desktop/entitlements-sidecar.plist \
      "$SIDECAR"
    echo "==> Ad-hoc signing app bundle with entitlements..."
    codesign --force --sign - \
      --entitlements claw-fleet-desktop/entitlements.plist \
      "$APP_BUNDLE"
  fi

  # 5. Create DMG
  DMG_DIR="target/$MODE/bundle/dmg"
  mkdir -p "$DMG_DIR"
  DMG_NAME="claw-fleet-${DEV_VERSION}.dmg"
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

  # 6. Build PKG installer
  PKG_DIR="target/$MODE/bundle/pkg"
  mkdir -p "$PKG_DIR"
  PKG_NAME="claw-fleet-${DEV_VERSION}.pkg"
  echo "==> Building PKG installer..."
  if [[ -n "$APPLE_INSTALLER_IDENTITY" ]]; then
    pkgbuild --component "$APP_BUNDLE" \
      --identifier "com.hoveychen.claw-fleet" \
      --version "$DEV_VERSION" \
      --install-location "/Applications" \
      --sign "$APPLE_INSTALLER_IDENTITY" \
      "$PKG_DIR/$PKG_NAME"
  else
    pkgbuild --component "$APP_BUNDLE" \
      --identifier "com.hoveychen.claw-fleet" \
      --version "$DEV_VERSION" \
      --install-location "/Applications" \
      "$PKG_DIR/$PKG_NAME"
  fi
  echo "==> PKG: $PKG_DIR/$PKG_NAME"
  open "$PKG_DIR/$PKG_NAME"
fi

# 7. Notarize (optional)
if [[ "$NOTARIZE" == true ]]; then
  if [[ -z "${APP_STORE_CONNECT_KEY:-}" ]]; then
    echo "ERROR: --notarize requires APP_STORE_CONNECT_KEY in scripts/signing.local"
    exit 1
  fi

  echo "==> Preparing for notarization..."
  NOTARIZE_TMP=$(mktemp -d)
  trap 'rm -rf "$NOTARIZE_TMP"; for toml in claw-fleet-core/Cargo.toml claw-fleet-desktop/Cargo.toml fleet-cli/Cargo.toml; do mv "${toml}.bak" "$toml" 2>/dev/null || true; done' EXIT

  echo "$APP_STORE_CONNECT_KEY" | base64 --decode > "$NOTARIZE_TMP/AuthKey_${APP_STORE_CONNECT_KEY_ID}.p8"

  echo "==> Creating zip for notarization..."
  ditto -c -k --keepParent "$APP_BUNDLE" "$NOTARIZE_TMP/app.zip"

  echo "==> Submitting to Apple notary service..."
  xcrun notarytool submit "$NOTARIZE_TMP/app.zip" \
    --key "$NOTARIZE_TMP/AuthKey_${APP_STORE_CONNECT_KEY_ID}.p8" \
    --key-id "$APP_STORE_CONNECT_KEY_ID" \
    --issuer "$APP_STORE_CONNECT_ISSUER_ID" \
    --wait --timeout 15m

  echo "==> Stapling notarization ticket to app..."
  xcrun stapler staple "$APP_BUNDLE"

  echo "==> Re-creating DMG with notarized app..."
  rm -f "$DMG_DIR"/*.dmg
  DMG_NAME="claw-fleet-${DEV_VERSION}.dmg"
  DMG_STAGING=$(mktemp -d)
  cp -R "$APP_BUNDLE" "$DMG_STAGING/"
  ln -s /Applications "$DMG_STAGING/Applications"
  hdiutil create -volname "Claw Fleet" \
    -srcfolder "$DMG_STAGING" \
    -ov -format UDZO \
    "$DMG_DIR/$DMG_NAME"
  rm -rf "$DMG_STAGING"

  echo "==> Re-creating PKG with notarized app..."
  rm -f "$PKG_DIR"/*.pkg
  PKG_NAME="claw-fleet-${DEV_VERSION}.pkg"
  if [[ -n "${APPLE_INSTALLER_IDENTITY:-}" ]]; then
    pkgbuild --component "$APP_BUNDLE" \
      --identifier "com.hoveychen.claw-fleet" \
      --version "$DEV_VERSION" \
      --install-location "/Applications" \
      --sign "$APPLE_INSTALLER_IDENTITY" \
      "$PKG_DIR/$PKG_NAME"
  else
    pkgbuild --component "$APP_BUNDLE" \
      --identifier "com.hoveychen.claw-fleet" \
      --version "$DEV_VERSION" \
      --install-location "/Applications" \
      "$PKG_DIR/$PKG_NAME"
  fi

  echo "==> Notarizing PKG..."
  xcrun notarytool submit "$PKG_DIR/$PKG_NAME" \
    --key "$NOTARIZE_TMP/AuthKey_${APP_STORE_CONNECT_KEY_ID}.p8" \
    --key-id "$APP_STORE_CONNECT_KEY_ID" \
    --issuer "$APP_STORE_CONNECT_ISSUER_ID" \
    --wait --timeout 15m

  xcrun stapler staple "$PKG_DIR/$PKG_NAME"
  echo "==> PKG notarized: $PKG_DIR/$PKG_NAME"

  echo "==> Notarization complete!"
fi

echo ""
echo "Done! Version: $DEV_VERSION"
echo "App bundle: target/$MODE/bundle/"
