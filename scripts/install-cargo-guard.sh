#!/usr/bin/env bash
# Install (or remove) the cargo concurrency gate as a shim on PATH.
#
# The shim must sit in a PATH entry that precedes ~/.cargo/bin, otherwise the
# real cargo wins the lookup and the gate never runs. On this machine that is
# ~/.local/bin. The shim is a symlink back into the repo so a `git pull` takes
# effect without reinstalling — which also means the repo must stay where it is.
#
# Usage:
#   scripts/install-cargo-guard.sh            install (or repoint) the shim
#   scripts/install-cargo-guard.sh --uninstall  remove it
#   scripts/install-cargo-guard.sh --status     report what is installed
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
GUARD="$SCRIPT_DIR/cargo-jobs-guard.sh"
SHIM_DIR="${FLEET_CARGO_SHIM_DIR:-$HOME/.local/bin}"
SHIM="$SHIM_DIR/cargo"

real_cargo_dir() {
  [[ -x "$HOME/.cargo/bin/cargo" ]] && printf '%s\n' "$HOME/.cargo/bin"
}

status() {
  echo "shim path : $SHIM"
  if [[ -L "$SHIM" ]]; then
    echo "installed : yes -> $(readlink "$SHIM")"
  elif [[ -e "$SHIM" ]]; then
    echo "installed : NO — a non-symlink file already occupies that path"
  else
    echo "installed : no"
  fi
  echo "PATH cargo: $(command -v cargo || echo '<none>')"
  echo "slot store: ${FLEET_CARGO_SLOT_ROOT:-/tmp/claw-fleet-cargo-slots-$(id -u)}"
}

case "${1:-}" in
  --status)
    status
    exit 0
    ;;
  --uninstall)
    if [[ -L "$SHIM" ]]; then
      rm "$SHIM"
      echo "==> Removed $SHIM"
    else
      # Refusing rather than deleting: if it is not our symlink, it is not ours
      # to remove.
      echo "==> Nothing to remove ($SHIM is not a symlink installed by us)"
    fi
    exit 0
    ;;
  "") ;;
  *)
    echo "unknown argument: $1" >&2
    exit 2
    ;;
esac

[[ -x "$GUARD" ]] || { echo "==> $GUARD missing or not executable" >&2; exit 1; }

# Never clobber a real binary that happens to live at the shim path.
if [[ -e "$SHIM" && ! -L "$SHIM" ]]; then
  echo "==> $SHIM already exists and is not a symlink — refusing to overwrite." >&2
  echo "    Move it aside and re-run." >&2
  exit 1
fi

mkdir -p "$SHIM_DIR"
ln -sfn "$GUARD" "$SHIM"
echo "==> Installed shim: $SHIM -> $GUARD"

# ── Verify the shim actually wins the PATH lookup ────────────────────────────
# Installing it is not the same as it being used; a PATH that puts ~/.cargo/bin
# first silently gives you the ungated cargo.
resolved="$(command -v cargo || true)"
if [[ "$resolved" != "$SHIM" ]]; then
  echo "==> WARNING: PATH still resolves cargo to $resolved" >&2
  echo "    $SHIM_DIR must come before $(real_cargo_dir) in PATH for the gate to apply." >&2
  exit 1
fi

echo "==> PATH resolves cargo to the shim. Gate is active."
echo "    Disable for one command with FLEET_CARGO_GUARD=0."
status
