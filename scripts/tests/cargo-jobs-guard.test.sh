#!/usr/bin/env bash
# Tests for scripts/cargo-jobs-guard.sh.
#
# No real compilation happens here: a fake `cargo` on PATH records the argv and
# the environment it was handed, so every assertion is about the guard's own
# decisions (gate / passthrough / queue / reap) rather than about rustc.
#
# The guard derives its slot store from TMPDIR, so pointing TMPDIR at a scratch
# directory isolates each test from the machine's real slots for free.
#
# Usage: scripts/tests/cargo-jobs-guard.test.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
GUARD="$SCRIPT_DIR/../cargo-jobs-guard.sh"

pass=0
fail=0

ok()   { printf '  ok   %s\n' "$1"; pass=$(( pass + 1 )); }
bad()  { printf '  FAIL %s\n     %s\n' "$1" "${2:-}"; fail=$(( fail + 1 )); }

# Each test gets a fresh sandbox: its own TMPDIR (hence its own slot store),
# its own fake cargo, its own log.
setup() {
  SANDBOX="$(mktemp -d)"
  mkdir -p "$SANDBOX/bin" "$SANDBOX/tmp"
  FAKE_LOG="$SANDBOX/cargo.log"
  cat > "$SANDBOX/bin/cargo" <<'FAKE'
#!/usr/bin/env bash
{
  echo "argv: $*"
  echo "held: ${CARGO_JOBS_GUARD_HELD:-<unset>}"
  echo "jobs: ${CARGO_BUILD_JOBS:-<unset>}"
} >> "$FAKE_LOG"
[[ -n "${FAKE_SLEEP:-}" ]] && sleep "$FAKE_SLEEP"
exit "${FAKE_EXIT:-0}"
FAKE
  chmod +x "$SANDBOX/bin/cargo"
  export FAKE_LOG
  export TMPDIR="$SANDBOX/tmp"
  SLOT_ROOT="$TMPDIR/claw-fleet-cargo-slots"
}

teardown() {
  rm -rf "$SANDBOX"
  unset FAKE_SLEEP FAKE_EXIT
}

run_guard() {
  PATH="$SANDBOX/bin:$PATH" FLEET_CARGO_GUARD_QUIET=1 "$GUARD" "$@"
}

# ── The gate must not fire on cheap or tool-driven invocations ───────────────

setup
run_guard fmt >/dev/null 2>&1
if grep -q 'held: <unset>' "$FAKE_LOG"; then
  ok "cheap subcommand (fmt) passes through ungated"
else
  bad "cheap subcommand (fmt) passes through ungated" "$(cat "$FAKE_LOG")"
fi
teardown

setup
run_guard metadata --format-version 1 >/dev/null 2>&1
if grep -q 'held: <unset>' "$FAKE_LOG"; then
  ok "metadata passes through ungated"
else
  bad "metadata passes through ungated" "$(cat "$FAKE_LOG")"
fi
teardown

# Regression guard: --message-format trails the subcommand, so a scan that
# stops at the first non-flag argument never sees it and would gate
# rust-analyzer's every-save check — freezing the editor with no visible cause.
setup
run_guard check --message-format=json >/dev/null 2>&1
if grep -q 'held: <unset>' "$FAKE_LOG"; then
  ok "check --message-format=json is exempt (rust-analyzer path)"
else
  bad "check --message-format=json is exempt (rust-analyzer path)" "$(cat "$FAKE_LOG")"
fi
teardown

setup
run_guard build --message-format json >/dev/null 2>&1
if grep -q 'held: <unset>' "$FAKE_LOG"; then
  ok "--message-format as a separate argument is exempt too"
else
  bad "--message-format as a separate argument is exempt too" "$(cat "$FAKE_LOG")"
fi
teardown

# ── The gate must fire on compile-heavy invocations ──────────────────────────

setup
run_guard build >/dev/null 2>&1
if grep -q 'held: 1' "$FAKE_LOG"; then
  ok "build is gated"
else
  bad "build is gated" "$(cat "$FAKE_LOG")"
fi
teardown

setup
run_guard test -p some-crate >/dev/null 2>&1
if grep -q 'held: 1' "$FAKE_LOG"; then
  ok "test is gated"
else
  bad "test is gated" "$(cat "$FAKE_LOG")"
fi
teardown

# ── Recursion: a build script re-entering the shim must not wait on the slot
#    its own parent is holding. That is a self-deadlock, not contention. ──────

setup
CARGO_JOBS_GUARD_HELD=1 run_guard build >/dev/null 2>&1
if grep -q 'argv: build' "$FAKE_LOG" && [[ ! -d "$SLOT_ROOT/slot-1" ]]; then
  ok "nested cargo (CARGO_JOBS_GUARD_HELD set) passes through without queueing"
else
  bad "nested cargo passes through without queueing" "$(cat "$FAKE_LOG")"
fi
teardown

# ── Slot release ─────────────────────────────────────────────────────────────

setup
run_guard build >/dev/null 2>&1
if [[ ! -d "$SLOT_ROOT/slot-1" ]]; then
  ok "slot is released when cargo exits"
else
  bad "slot is released when cargo exits" "slot-1 still present"
fi
teardown

setup
FAKE_EXIT=42 run_guard build >/dev/null 2>&1
status=$?
if (( status == 42 )) && [[ ! -d "$SLOT_ROOT/slot-1" ]]; then
  ok "cargo's exit status propagates and the slot is still released"
else
  bad "cargo's exit status propagates and the slot is still released" "status=$status"
fi
teardown

# ── Queueing: a second heavy cargo waits while the only slot is held ─────────

setup
FLEET_CARGO_SLOTS=1 FAKE_SLEEP=6 \
  PATH="$SANDBOX/bin:$PATH" FLEET_CARGO_GUARD_QUIET=1 "$GUARD" build >/dev/null 2>&1 &
holder=$!
# Let the background guard actually take the slot before racing it.
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [[ -d "$SLOT_ROOT/slot-1" ]] && break
  sleep 0.3
done
if [[ -d "$SLOT_ROOT/slot-1" ]]; then
  start=$(date +%s)
  FLEET_CARGO_SLOTS=1 FLEET_CARGO_MAX_WAIT=4 run_guard build >/dev/null 2>&1
  elapsed=$(( $(date +%s) - start ))
  if (( elapsed >= 4 )); then
    ok "a second heavy cargo waits for the busy slot (waited ${elapsed}s)"
  else
    bad "a second heavy cargo waits for the busy slot" "returned after ${elapsed}s"
  fi
else
  bad "a second heavy cargo waits for the busy slot" "holder never took slot-1"
fi
wait "$holder" 2>/dev/null
teardown

# ── Bounded wait: the gate proceeds rather than wedging the session forever ──

setup
mkdir -p "$SLOT_ROOT/slot-1"
# A live owner that is not going to finish: the wait must time out, not hang.
sleep 30 &
squatter=$!
echo "$squatter $PWD held" > "$SLOT_ROOT/slot-1/owner"
start=$(date +%s)
FLEET_CARGO_SLOTS=1 FLEET_CARGO_MAX_WAIT=3 \
  PATH="$SANDBOX/bin:$PATH" "$GUARD" build >/dev/null 2>&1
elapsed=$(( $(date +%s) - start ))
if grep -q 'argv: build' "$FAKE_LOG" && (( elapsed >= 3 && elapsed < 20 )); then
  ok "wait is bounded — proceeds after FLEET_CARGO_MAX_WAIT (${elapsed}s)"
else
  bad "wait is bounded" "elapsed=${elapsed}s log=$(cat "$FAKE_LOG")"
fi
kill "$squatter" 2>/dev/null
wait "$squatter" 2>/dev/null
teardown

# ── Stale reap: debris from a crash must not shrink capacity permanently ─────

setup
mkdir -p "$SLOT_ROOT/slot-1"
# A pid that is certainly not running: spawn one and reap it first.
sleep 0 & dead=$!
wait "$dead" 2>/dev/null
echo "$dead $PWD stale" > "$SLOT_ROOT/slot-1/owner"
start=$(date +%s)
FLEET_CARGO_SLOTS=1 FLEET_CARGO_MAX_WAIT=30 run_guard build >/dev/null 2>&1
elapsed=$(( $(date +%s) - start ))
if grep -q 'held: 1' "$FAKE_LOG" && (( elapsed < 10 )); then
  ok "a slot whose owner died is reaped, not waited on (${elapsed}s)"
else
  bad "a slot whose owner died is reaped" "elapsed=${elapsed}s log=$(cat "$FAKE_LOG")"
fi
teardown

# ── Job cap ──────────────────────────────────────────────────────────────────

setup
FLEET_BUILD_JOBS=3 run_guard build >/dev/null 2>&1
if grep -q 'jobs: 3' "$FAKE_LOG"; then
  ok "FLEET_BUILD_JOBS overrides the computed job count"
else
  bad "FLEET_BUILD_JOBS overrides the computed job count" "$(cat "$FAKE_LOG")"
fi
teardown

setup
run_guard build >/dev/null 2>&1
jobs_line="$(grep '^jobs: ' "$FAKE_LOG" | head -1)"
jobs_val="${jobs_line#jobs: }"
if [[ "$jobs_val" =~ ^[0-9]+$ ]] && (( jobs_val >= 2 )); then
  ok "a computed job cap is exported to cargo (jobs=$jobs_val)"
else
  bad "a computed job cap is exported to cargo" "$jobs_line"
fi
teardown

# ── Disable switch ───────────────────────────────────────────────────────────

setup
FLEET_CARGO_GUARD=0 run_guard build >/dev/null 2>&1
if grep -q 'held: <unset>' "$FAKE_LOG"; then
  ok "FLEET_CARGO_GUARD=0 disables the gate entirely"
else
  bad "FLEET_CARGO_GUARD=0 disables the gate entirely" "$(cat "$FAKE_LOG")"
fi
teardown

printf '\n%d passed, %d failed\n' "$pass" "$fail"
(( fail == 0 ))
