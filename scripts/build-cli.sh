#!/usr/bin/env bash
# Build and install the Phase 2–4 CLI binaries (fleet-task, fleet, fleet-hooks-server)
# into a user-PATH location so you can dogfood the new task lifecycle without
# the desktop app.
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
#   - fleet-task              (Phase 2+ standalone task binary, owns lifecycle)
#   - fleet                   (Phase 4 CLI: `fleet serve` + `fleet task <cmd>`)
#   - fleet-hooks-server      (Phase 4: hook endpoints only, no task lifecycle)

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
cargo build $CARGO_FLAGS -p fleet-task -p fleet-cli -p fleet-hooks-server

TARGET_DIR="$ROOT_DIR/target/$PROFILE"
for bin in fleet-task fleet-cli fleet-hooks-server; do
  src="$TARGET_DIR/$bin"
  if [[ ! -x "$src" ]]; then
    echo "Error: expected binary not found at $src" >&2
    exit 1
  fi
done

if [[ $DO_INSTALL -eq 0 ]]; then
  echo ""
  echo "==> Build done. Skipping install (--no-install). Binaries are at:"
  printf '    %s\n' "$TARGET_DIR/fleet-task" "$TARGET_DIR/fleet-cli" "$TARGET_DIR/fleet-hooks-server"
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
link_or_replace "$TARGET_DIR/fleet-task"          "$INSTALL_DIR/fleet-task"         "fleet-task"
link_or_replace "$TARGET_DIR/fleet-cli"           "$INSTALL_DIR/fleet-cli"          "fleet-cli"
# `fleet` is the canonical name end-users see (the bundled sidecar is also
# called `fleet`); alias fleet-cli under that name so `fleet task <cmd>` works.
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

  # 1. Start a real task — opens the ratatui TUI in this terminal (P-item
  #    list + status bar with task / port / pid; `q` to quit). Requires
  #    `claude` on PATH and a git repo workspace.
  fleet-task new --workspace ~/code/my-repo --prompt "explore the codebase"

  # 2. Same task, headless (no TUI). Useful when desktop or another tool
  #    drives it; foreground waits on SIGINT/SIGTERM.
  fleet-task new --workspace ~/code/my-repo --prompt "..." --no-tui

  # 3. Smoke-test plumbing without `claude` installed (`sleep 60` stands in
  #    for the master subprocess so registry / HTTP / shutdown all exercise):
  FLEET_TASK_FAKE_LAUNCHER=1 \
    fleet-task new --workspace ~/code/my-repo --prompt "smoke" --no-tui &

  # 4. From another terminal: list all tasks (shell-friendly table) or
  #    pop a TUI picker that lets you Enter-to-resume any of them:
  fleet task list                  # `--json` for jq pipelines
  fleet-task list                  # ratatui picker; Enter to resume into single-task TUI

  # 5. Inspect / drive the task via the CLI HTTP path:
  TASK_ID=$(fleet task list --json | jq -r '.[0].id')
  fleet task get-plan "$TASK_ID"
  fleet task dispatch "$TASK_ID" <p-item-id>      # HTTP → fleet-task LocalDispatcher
  fleet task mark-done "$TASK_ID" <p-item-id> "summary"

  # 6. Standalone hook server (was 'fleet serve' supervisor + hooks; now just
  #    hooks — no LaunchAgent, run it on demand when claude needs guard etc.):
  fleet-hooks-server --port 0

USAGE
