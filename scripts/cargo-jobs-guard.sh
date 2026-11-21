#!/usr/bin/env bash
# Machine-global concurrency gate for cargo.
#
# Installed as a `cargo` shim ahead of ~/.cargo/bin on PATH (see
# scripts/install-cargo-guard.sh), this holds a slot for the duration of any
# compile-heavy cargo invocation and runs the real cargo under it. When a
# `fleet` that has `fleet resource run` is found, the slot is a unit of the
# machine-global resource `cargo` in Fleet's resource queue (capacity =
# FLEET_CARGO_SLOTS), so cargo shows up — and can be reordered — next to every
# other queued resource. Without one it falls back to the mkdir slot store
# below. The problem it
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
#   FLEET_CARGO_ALLOW_CLEAN=1      permit `cargo clean` against a shared target dir
#   FLEET_CARGO_FLEET_BIN=<path>   use this fleet for the resource queue; `none`
#                                  forces the mkdir slot store (tests use both)

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

  case "$(subcommand_of "$@")" in
    build|b|test|t|check|c|clippy|run|r|bench|doc|d|rustc|install|nextest) return 0 ;;
    *) return 1 ;;
  esac
}

# First non-flag argument, i.e. the subcommand. Empty for a bare `cargo` or
# `cargo --version`.
subcommand_of() {
  local arg
  for arg in "$@"; do
    case "$arg" in
      -*) continue ;;
      *) printf '%s\n' "$arg"; return ;;
    esac
  done
}

# ── Refuse `cargo clean` when the target dir is shared ───────────────────────
# `build.target-dir` in this repo's .cargo/config.toml points every checkout
# and every `.worktrees/<id>/` at one shared target dir. That is a large win —
# cargo skips already-built dependency units outright instead of recompiling
# them per worktree — but it turns `cargo clean` from a local reset into a
# machine-wide wipe. Measured 2026-09-06: 43G in the shared dir with a dozen
# concurrent plans reading from it. Cargo has no "clean only my share", so the
# only protection is not running it.
if [[ "$(subcommand_of "$@")" == "clean" && "${FLEET_CARGO_ALLOW_CLEAN:-0}" != "1" ]]; then
  # Ask cargo rather than reimplementing the lookup: the effective target dir
  # comes from CARGO_TARGET_DIR, build.target-dir, or a config file found by
  # walking up from cwd, and a hand-rolled copy of that would drift.
  guard_meta="$("$REAL_CARGO" metadata --format-version 1 --no-deps 2>/dev/null)"
  guard_tdir="$(printf '%s' "$guard_meta" | grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)"
  guard_wroot="$(printf '%s' "$guard_meta" | grep -o '"workspace_root":"[^"]*"' | head -1 | cut -d'"' -f4)"
  # Fail open when metadata does not resolve (not a cargo project, unreadable
  # manifest): blocking a clean nobody can explain is worse than allowing one.
  if [[ -n "$guard_tdir" && -n "$guard_wroot" && "$guard_tdir" != "$guard_wroot"/* ]]; then
    cat >&2 <<EOF
==> cargo-jobs-guard: refusing \`cargo clean\`.

    target dir:     $guard_tdir
    this workspace: $guard_wroot

    The target dir is outside this workspace, so it is shared with every other
    checkout and worktree on this machine. Cleaning it discards their build
    artifacts too and forces a full rebuild in every concurrent session.

    Override deliberately with:  FLEET_CARGO_ALLOW_CLEAN=1 cargo clean
EOF
    exit 1
  fi
fi

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

# ── Preferred gate: Fleet's resource queue ──────────────────────────────────
# A fleet counts only if it really has `resource run`. Exit status is not
# enough to tell: older builds answer an unknown subcommand with exit 0 and no
# output, which would "succeed" without ever running cargo.
fleet_usable() {
  [[ -x "$1" ]] && "$1" resource run --help 2>/dev/null | grep -q -- '--need'
}

find_fleet() {
  local want="${FLEET_CARGO_FLEET_BIN:-}"
  if [[ "$want" == "none" ]]; then
    return 1
  fi
  if [[ -n "$want" ]]; then
    fleet_usable "$want" && { printf '%s\n' "$want"; return 0; }
    return 1
  fi
  local dir candidate canon seen=":"
  local IFS=:
  for candidate in $(for dir in $PATH; do printf '%s:' "$dir/fleet"; done) "$HOME/.fleet/bin/fleet"; do
    [[ -x "$candidate" ]] || continue
    canon="$(canonical "$candidate")"
    [[ "$seen" == *":$canon:"* ]] && continue
    seen="$seen$canon:"
    if fleet_usable "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

# The capacity of `cargo` as recorded in the queue's state file, or nothing.
# Read straight from the file to skip a locked write on every cargo call when
# it already matches; any parse miss just means one redundant `resource set`.
recorded_cargo_capacity() {
  local state="${FLEET_HOME:-$HOME}/.fleet/resources/state.json"
  [[ -r "$state" ]] || return 0
  awk '/^    "cargo": \{/ { getline; if ($1 == "\"capacity\":") { gsub(/[^0-9]/, "", $2); print $2 }; exit }' "$state" 2>/dev/null
}

FLEET_BIN="$(find_fleet || true)"
if [[ -n "$FLEET_BIN" ]]; then
  if [[ "$(recorded_cargo_capacity)" != "$SLOTS" ]]; then
    # Refused in an incognito session (only `resource run` is allowed there);
    # the queue then keeps whatever capacity it has.
    "$FLEET_BIN" resource set cargo --capacity "$SLOTS" >/dev/null 2>&1 || true
  fi
  export CARGO_JOBS_GUARD_HELD=1
  CARGO_BUILD_JOBS="$(compute_jobs)"
  export CARGO_BUILD_JOBS

  # `fleet resource run` exits with its own codes when the command never
  # starts (75 queue timeout, 76 cancelled, 64 refused), and cargo's exit code
  # otherwise — but `cargo run` passes its program's code through, so a 75
  # could be either. A marker the command drops as it starts tells them apart.
  marker_dir="${FLEET_CARGO_SLOT_ROOT:-/tmp/claw-fleet-cargo-slots-$(id -u)}"
  mkdir -p "$marker_dir" 2>/dev/null || true
  started="$marker_dir/started.$$"
  rm -f "$started" 2>/dev/null
  trap 'rm -f "$started" 2>/dev/null' EXIT
  # shellcheck disable=SC2016
  "$FLEET_BIN" resource run --need cargo --max-wait "${FLEET_CARGO_MAX_WAIT:-1800}" \
    -- /bin/sh -c ': > "$0"; exec "$@"' "$started" "$REAL_CARGO" "$@"
  status=$?
  if [[ -e "$started" ]]; then
    exit "$status"
  fi
  case "$status" in
    # Cancelled by hand in the queue: the user meant it not to run.
    76) exit "$status" ;;
  esac
  # Interrupted while queued (128 + signal): stop, do not build.
  (( status > 128 )) && exit "$status"
  # Never let the gate deadlock or break a build: a queue timeout proceeds
  # like the old slot wait did, and a refusal (e.g. this command already runs
  # under a grant that does not include `cargo`) or a broken fleet runs
  # cargo ungated rather than not at all.
  if [[ "${FLEET_CARGO_GUARD_QUIET:-0}" != "1" ]]; then
    if (( status == 75 )); then
      echo "==> cargo-jobs-guard: waited ${FLEET_CARGO_MAX_WAIT:-1800}s for the cargo resource, proceeding anyway" >&2
    else
      echo "==> cargo-jobs-guard: fleet resource run did not start cargo (exit $status), running it ungated" >&2
    fi
  fi
  "$REAL_CARGO" "$@"
  exit $?
fi

# ── Fallback gate: acquire one of N mkdir slots ──────────────────────────────
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
