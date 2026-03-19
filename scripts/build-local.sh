#!/usr/bin/env bash
# Build Claude Fleet locally (GUI app + fleet CLI sidecar).
# Usage: ./scripts/build-local.sh [--debug]
set -euo pipefail

MODE="release"
CARGO_FLAG="--release"
if [[ "${1:-}" == "--debug" ]]; then
  MODE="debug"
  CARGO_FLAG=""
fi

# ── Generate dev version ──────────────────────────────────────────────────────
# Format: {YY}.{M}.{D}-dev.{epoch}  e.g. 25.3.19-dev.1742371234
# Valid SemVer — no leading zeros allowed in numeric components
YY=$(date +%y)
MM=$((10#$(date +%m)))   # strip leading zero: 03 → 3
DD=$((10#$(date +%d)))   # strip leading zero: 09 → 9
DEV_VERSION="${YY}.${MM}.${DD}-dev.$(date +%s)"
echo "==> Dev version: $DEV_VERSION"

CARGO_TOML="src-tauri/Cargo.toml"
CARGO_TOML_BAK="${CARGO_TOML}.bak"

# Patch Cargo.toml and restore on exit
cp "$CARGO_TOML" "$CARGO_TOML_BAK"
trap 'mv "$CARGO_TOML_BAK" "$CARGO_TOML"' EXIT

sed -i.tmp "s/^version = \".*\"/version = \"${DEV_VERSION}\"/" "$CARGO_TOML"
rm -f "${CARGO_TOML}.tmp"

# Detect native target triple
TARGET=$(rustc -vV | sed -n 's|host: ||p')
echo "==> Target: $TARGET  (mode: $MODE)"

# 1. Build fleet CLI (native)
echo "==> Building fleet CLI (native)..."
cargo build $CARGO_FLAG --bin fleet-cli --manifest-path "$CARGO_TOML"

# 2. Copy compiled binary into binaries/ so Tauri bundles the real binary
mkdir -p src-tauri/binaries
SRC="src-tauri/target/$MODE/fleet-cli"
DST="src-tauri/binaries/fleet-$TARGET"
cp "$SRC" "$DST"
chmod +x "$DST"
echo "==> Copied fleet CLI → $DST"

# 3. Build Tauri app (frontend + Rust main binary)
echo "==> Building Tauri app..."
npm run tauri build

echo ""
echo "Done! Version: $DEV_VERSION"
echo "App bundle: src-tauri/target/$TARGET/$MODE/bundle/"
