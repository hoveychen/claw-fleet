#!/usr/bin/env bash
# Build and install the CLI binaries (fleet, fleet-hooks-server) into a
# user-PATH location so you can dogfood them without the desktop app.
#
# Defaults:
#   - profile = debug (fast incremental builds)
#   - install dir = ~/.local/bin (symlinks into target/<profile>/)
#
# Usage:
#   ./scripts/build-cli.sh                       # debug build, symlink to ~/.local/bin
#   ./scripts/build-cli.sh --release             # release build (slower; optimised)
#   ./scripts/build-cli.sh --install-dir DIR     # custom install dir
#   ./scripts/build-cli.sh --no-install          # only build, don't symlink
#
# After install the following are on PATH:
#   - fleet                   (`fleet serve` + wiki / plan / session / guard etc.)
#   - fleet-hooks-server      (hook endpoints only)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

PROFILE="debug"
CARGO_FLAGS=""
INSTALL_DIR="${HOME}/.local/bin"
DO_INSTALL=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      PROFILE="release"
      CARGO_FLAGS="--release"
      shift
      ;;
    --debug)
      PROFILE="debug"
      CARGO_FLAGS=""
      shift
      ;;
    --install-dir)
      INSTALL_DIR="$2"
      shift 2
      ;;
    --no-install)
      DO_INSTALL=0
      shift
      ;;
    -h|--help)
      sed -n '2,20p' "$0"
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 1
      ;;
  esac
done

echo "==> Building Phase 2–4 CLI binaries ($PROFILE profile)"
# shellcheck disable=SC2086
cargo build $CARGO_FLAGS -p fleet-cli -p fleet-hooks-server

TARGET_DIR="$ROOT_DIR/target/$PROFILE"
for bin in fleet-cli fleet-hooks-server; do
  src="$TARGET_DIR/$bin"
  if [[ ! -x "$src" ]]; then
    echo "Error: expected binary not found at $src" >&2
    exit 1
  fi
done

if [[ $DO_INSTALL -eq 0 ]]; then
  echo ""
  echo "==> Build done. Skipping install (--no-install). Binaries are at:"
  printf '    %s\n' "$TARGET_DIR/fleet-cli" "$TARGET_DIR/fleet-hooks-server"
  exit 0
fi

mkdir -p "$INSTALL_DIR"

link_or_replace() {
  local src="$1" dst="$2" label="$3"
  if [[ -L "$dst" ]] && [[ "$(readlink "$dst")" == "$src" ]]; then
    printf '    %-22s already linked\n' "$label"
    return
  fi
  ln -sf "$src" "$dst"
  printf '    %-22s -> %s\n' "$label" "$dst"
}

echo ""
echo "==> Installing symlinks into $INSTALL_DIR"
link_or_replace "$TARGET_DIR/fleet-cli"           "$INSTALL_DIR/fleet-cli"          "fleet-cli"
# `fleet` is the canonical name end-users see (the bundled sidecar is also
# called `fleet`); alias fleet-cli under that name.
link_or_replace "$TARGET_DIR/fleet-cli"           "$INSTALL_DIR/fleet"              "fleet (alias)"
link_or_replace "$TARGET_DIR/fleet-hooks-server"  "$INSTALL_DIR/fleet-hooks-server" "fleet-hooks-server"

if ! echo ":$PATH:" | grep -q ":$INSTALL_DIR:"; then
  cat <<EOF

⚠  $INSTALL_DIR is not on your PATH. Add to your shell rc:
    export PATH="$INSTALL_DIR:\$PATH"
EOF
fi

cat <<'USAGE'

==> Done. Quick dogfood path:

  # Serve the hook/probe HTTP API (guard / elicitation / wiki / plans …):
  fleet serve --port 0

  # Standalone hook server (hook endpoints only, run on demand):
  fleet-hooks-server --port 0

USAGE
