#!/usr/bin/env bash
# Machine-global concurrency gate for cargo.
#
# Installed as a `cargo` shim ahead of ~/.cargo/bin on PATH (see
# scripts/install-cargo-guard.sh), this holds a slot for the duration of any
# compile-heavy cargo invocation and then execs the real cargo. The problem it
# solves is cross-*session*, not cross-target: several agent sessions each type
# their own `cargo check`, cargo defaults every one of them to jobs = ncpu, and
# the machine ends up carrying ncpu × sessions concurrent rustc. Rule 3's
# worktree workflow multiplies that further because every worktree compiles
# into its own target/.
#
# Design constraints that shaped this:
#   * The slot store is machine-global (under TMPDIR), never under target/ —
#     a repo-local gate cannot see a sibling worktree's build, which is exactly
#     the overlap that happens in practice.
#   * `mkdir` is the atomic primitive because macOS ships no flock(1).
#   * Waiting is bounded. A gate that can deadlock a session is worse than one
#     that occasionally lets an extra build through, so on timeout we proceed
#     rather than block forever.
#   * Tool-driven cargo (rust-analyzer, IDEs) is exempt — see is_heavy().
#     Queueing those would freeze the editor with no visible cause.
#
# Env knobs:
#   FLEET_CARGO_GUARD=0            disable entirely (straight passthrough)
#   FLEET_CARGO_SLOTS=<n>          concurrent compile-heavy cargos (default 2)
#   FLEET_CARGO_MAX_WAIT=<secs>    give up waiting and proceed (default 1800)
#   FLEET_BUILD_JOBS=<n>           force CARGO_BUILD_JOBS, skips the memory calc
#   FLEET_CARGO_GUARD_QUIET=1      suppress the "waiting for a slot" notices
#   FLEET_CARGO_SLOT_ROOT=<dir>    relocate the slot store (tests use this)

set -uo pipefail

GUARD_SELF="${BASH_SOURCE[0]}"

# ── Locate the real cargo ────────────────────────────────────────────────────
# Resolving by identity rather than by a hardcoded path: the shim is normally
# installed as ~/.local/bin/cargo, so a naive `command -v cargo` would find
# *us* and recurse. Compare canonical paths and take the first PATH entry that
# is not this script.
canonical() {
  local p="$1"
  if [[ -e "$p" ]]; then
    (cd "$(dirname "$p")" 2>/dev/null && printf '%s/%s\n' "$(pwd -P)" "$(basename "$p")")
  fi
}

SELF_CANON="$(canonical "$GUARD_SELF")"

find_real_cargo() {
  local candidate canon
  local IFS=:
  for dir in $PATH; do
    candidate="$dir/cargo"
    [[ -x "$candidate" ]] || continue
    canon="$(canonical "$candidate")"
    # Skip both the shim itself and the symlink pointing at it.
    [[ "$canon" == "$SELF_CANON" ]] && continue
    if [[ -L "$candidate" ]]; then
      local target
      target="$(readlink "$candidate")"
      [[ "$target" == "$GUARD_SELF" || "$(canonical "$target")" == "$SELF_CANON" ]] && continue
    fi
    printf '%s\n' "$candidate"
    return 0
  done
  # Last resort: rustup's own shim directory, which the shim never occupies.
  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    printf '%s\n' "$HOME/.cargo/bin/cargo"
    return 0
  fi
  return 1
}

REAL_CARGO="$(find_real_cargo || true)"
if [[ -z "$REAL_CARGO" ]]; then
  echo "cargo-jobs-guard: cannot find the real cargo on PATH" >&2
  exit 127
fi

# ── Decide whether this invocation is compile-heavy ──────────────────────────
# Only the subcommands that spawn a rustc fleet are gated. Everything else
# (fmt, metadata, tree, --version, locate-project, …) runs untouched: they are
# cheap, and several of them are on the critical path of tooling that must not
# stall.
is_heavy() {
  # Already inside a gated cargo — build scripts and cargo-* subcommands
  # re-enter the shim, and making a child wait on a slot its own parent holds
  # is a self-deadlock.
  [[ -n "${CARGO_JOBS_GUARD_HELD:-}" ]] && return 1
  [[ "${FLEET_CARGO_GUARD:-1}" == "0" ]] && return 1

  # Two passes on purpose. The exemption flags trail the subcommand
  # (`cargo check --message-format=json`), so a single loop that stops at the
  # first non-flag would never see them.
  local arg
  for arg in "$@"; do
    case "$arg" in
      # Machine-readable output means a tool is driving cargo (rust-analyzer
      # runs `cargo check --message-format=json` on every save). Blocking that
      # freezes the editor with nothing on screen to explain why.
      --message-format|--message-format=*) return 1 ;;
    esac
  done

  local subcmd=""
  for arg in "$@"; do
    case "$arg" in
      -*) continue ;;
      *) subcmd="$arg"; break ;;
    esac
  done

  case "$subcmd" in
    build|b|test|t|check|c|clippy|run|r|bench|doc|d|rustc|install|nextest) return 0 ;;
    *) return 1 ;;
  esac
}

if ! is_heavy "$@"; then
  exec "$REAL_CARGO" "$@"
fi

# ── Job cap: budget rustc against available memory, not core count ───────────
# The 2GB divisor comes from the measured 1224MB peak of a single rustc in this
# workspace, rounded up for headroom. The budget is then split across slots so
# that N concurrent cargos still add up to roughly one machine's worth of rustc
# rather than N machines' worth. Only ever lowers the count.
SLOTS="${FLEET_CARGO_SLOTS:-2}"
(( SLOTS < 1 )) && SLOTS=1

compute_jobs() {
  if [[ -n "${FLEET_BUILD_JOBS:-}" ]]; then
    printf '%s\n' "$FLEET_BUILD_JOBS"
    return
  fi
  # An explicit cap from the caller wins. build-local.sh computes its own and
  # exports it; recomputing here would silently overrule a deliberate choice.
  if [[ -n "${CARGO_BUILD_JOBS:-}" ]]; then
    printf '%s\n' "$CARGO_BUILD_JOBS"
    return
  fi
  local ncpu budget page free avail_gb
  ncpu="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"
  budget="$ncpu"
  if command -v vm_stat >/dev/null 2>&1; then
    page="$(vm_stat | sed -n '1s/.*page size of \([0-9]*\).*/\1/p')"
    free="$(vm_stat | awk '/Pages (free|inactive|speculative)/ {gsub(/\./,"",$NF); s+=$NF} END {print s+0}')"
    if [[ -n "$page" && "${free:-0}" -gt 0 ]]; then
      avail_gb=$(( free * page / 1073741824 ))
      (( avail_gb / 2 < budget )) && budget=$(( avail_gb / 2 ))
    fi
  fi
  local per_slot=$(( budget / SLOTS ))
  (( per_slot < 2 )) && per_slot=2
  (( per_slot > ncpu )) && per_slot="$ncpu"
  printf '%s\n' "$per_slot"
}

# ── Acquire one of N slots ───────────────────────────────────────────────────
# A fixed path, deliberately NOT under $TMPDIR. On macOS TMPDIR is a per-user
# launchd directory, but Fleet spawns its sessions detached — a session that
# does not inherit that environment falls back to /tmp and would then queue
# against a different, private slot store. Two stores means no gate at all, and
# the failure is silent. `id -u` keeps it per-user on shared machines.
SLOT_ROOT="${FLEET_CARGO_SLOT_ROOT:-/tmp/claw-fleet-cargo-slots-$(id -u)}"
mkdir -p "$SLOT_ROOT" 2>/dev/null || true
HELD_SLOT=""

release_slot() {
  if [[ -n "$HELD_SLOT" ]]; then
    rm -rf "$HELD_SLOT" 2>/dev/null || true
  fi
  HELD_SLOT=""
}

# A slot whose owner is gone is not held — it is debris from a Ctrl-C, a crash
# or a reboot. Without this sweep one interrupted build would permanently
# shrink the machine's compile capacity.
reap_stale() {
  local slot owner pid
  for slot in "$SLOT_ROOT"/slot-*; do
    [[ -d "$slot" ]] || continue
    owner="$(cat "$slot/owner" 2>/dev/null || true)"
    pid="${owner%% *}"
    if [[ -z "$pid" ]] || ! kill -0 "$pid" 2>/dev/null; then
      rm -rf "$slot" 2>/dev/null || true
    fi
  done
}

try_acquire() {
  local i slot
  for (( i = 1; i <= SLOTS; i++ )); do
    slot="$SLOT_ROOT/slot-$i"
    if mkdir "$slot" 2>/dev/null; then
      echo "$$ ${PWD} $(date '+%Y-%m-%d %H:%M:%S')" > "$slot/owner"
      HELD_SLOT="$slot"
      return 0
    fi
  done
  return 1
}

trap 'release_slot' EXIT
trap 'release_slot; exit 130' INT
trap 'release_slot; exit 143' TERM

MAX_WAIT="${FLEET_CARGO_MAX_WAIT:-1800}"
waited=0
notified=0
while ! try_acquire; do
  reap_stale
  try_acquire && break
  if (( waited >= MAX_WAIT )); then
    # Proceeding unqueued on purpose. A gate that can wedge a session forever
    # is a worse failure than one extra concurrent build.
    [[ "${FLEET_CARGO_GUARD_QUIET:-0}" == "1" ]] ||
      echo "==> cargo-jobs-guard: waited ${MAX_WAIT}s for a slot, proceeding anyway" >&2
    break
  fi
  if (( notified == 0 )) && [[ "${FLEET_CARGO_GUARD_QUIET:-0}" != "1" ]]; then
    echo "==> cargo-jobs-guard: all $SLOTS compile slots busy, waiting…" >&2
    notified=1
  fi
  sleep 2
  waited=$(( waited + 2 ))
done

export CARGO_JOBS_GUARD_HELD=1
CARGO_BUILD_JOBS="$(compute_jobs)"
export CARGO_BUILD_JOBS

"$REAL_CARGO" "$@"
status=$?
release_slot
exit "$status"
