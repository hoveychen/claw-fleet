#!/usr/bin/env bash
# check_spec_fidelity.sh — [REQ-045] CI spec-fidelity gate (top-level wrapper)
#
# This is the single entry point a commit / push / CI run invokes to assert the
# codebase is still faithful to the requirements registry. It chains a set of
# MECHANICALLY CHECKABLE sub-gates; if any one of them fails, the whole script
# exits 1 so the gate blocks the commit.
#
# Sub-gates (each prints a banner and a PASS/FAIL line):
#   1. req.json integrity — legal JSON; every REQ either carries an
#      implementation_file + a test pointer, OR is on the explicit exemption
#      list (infrastructure REQs 023/024/044/045 and any status=pending REQ).
#      The exemption list is printed in full so nothing is silently waved past.
#   2. annotation closure — delegates to scripts/verify_req_annotations.sh
#      (code [REQ-NNN] annotations <-> registry ids are consistent).
#   3. build + test — cargo build --workspace && cargo test --workspace.
#   4. deviation-ledger approval — ~/.fleet/deviations.jsonl (if it exists) has
#      no status=pending unapproved entries. Skipped (not an error) if absent.
#
# Out-of-CI (NOT counted toward exit code):
#   5. Auditor critical=0 — REQ-045's acceptance names an Auditor (P10) check.
#      A real Auditor run requires an independent Claude session and cannot run
#      in a headless CI runner. We DO NOT fake it. This step is printed as a
#      "manual / out-of-CI" reminder only; CI degrades this sub-check to a human
#      gate. This is an honest, declared limitation of REQ-045 in CI.
#
# Usage:  bash scripts/check_spec_fidelity.sh
# Exit:   0 = all in-CI gates green; 1 = at least one in-CI gate failed;
#         2 = harness/setup error (missing registry, no JSON parser, etc.)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REG="$ROOT/registry/req.json"
LEDGER="$ROOT/design/tasks-decisions-ledger.md"
DEVIATIONS="${FLEET_DEVIATIONS_FILE:-$HOME/.fleet/deviations.jsonl}"

# Infrastructure REQ ids that are exempt from the impl+test pointer requirement
# (they ARE the registry/CI machinery this task produces). Must stay in sync
# with INFRA_REQS in verify_req_annotations.sh.
INFRA_REQS="REQ-023 REQ-024 REQ-044 REQ-045"

overall=0          # final exit code accumulator
fail_summary=()    # human-readable list of failed gates

note_fail() { overall=1; fail_summary+=("$1"); }

banner() {
  echo
  echo "═══════════════════════════════════════════════════════════════════════"
  echo "  $1"
  echo "═══════════════════════════════════════════════════════════════════════"
}

# Pick a JSON tool once, reused by gate 1 and gate 4.
JSON_TOOL=""
if command -v python3 >/dev/null 2>&1; then
  JSON_TOOL="python3"
elif command -v jq >/dev/null 2>&1; then
  JSON_TOOL="jq"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Gate 1 — req.json integrity
# ─────────────────────────────────────────────────────────────────────────────
banner "Gate 1/4: req.json integrity (valid JSON + impl/test pointers or exemption)"

if [[ ! -f "$REG" ]]; then
  echo "FATAL: registry not found at $REG" >&2
  exit 2
fi

if [[ -z "$JSON_TOOL" ]]; then
  echo "FATAL: need python3 or jq to validate $REG, found neither" >&2
  exit 2
fi

# python3 is the most expressive; prefer it. Falls back to jq.
if [[ "$JSON_TOOL" == "python3" ]]; then
  gate1_out="$(python3 - "$REG" "$INFRA_REQS" <<'PY'
import json, sys
reg_path, infra_str = sys.argv[1], sys.argv[2]
infra = set(infra_str.split())
try:
    with open(reg_path) as f:
        data = json.load(f)
except Exception as e:
    print("JSON_INVALID\t%s" % e)
    sys.exit(0)
reqs = data.get("requirements")
if not isinstance(reqs, list):
    print("SCHEMA_INVALID\t.requirements is not a list")
    sys.exit(0)

exempt_infra, exempt_pending, ok, gaps = [], [], [], []
for r in reqs:
    rid = r.get("id", "<no-id>")
    status = r.get("status")
    has_impl = bool(r.get("implementation_file"))
    has_test = bool(r.get("test_file") or r.get("tests"))
    if rid in infra:
        exempt_infra.append("%s (infrastructure)" % rid)
    elif status == "pending":
        exempt_pending.append("%s (status=pending)" % rid)
    elif has_impl and has_test:
        ok.append(rid)
    else:
        miss = []
        if not has_impl: miss.append("implementation_file")
        if not has_test: miss.append("test")
        gaps.append("%s missing: %s" % (rid, ", ".join(miss)))

print("COUNT\t%d" % len(reqs))
print("OK\t%d" % len(ok))
for x in exempt_infra:   print("EXEMPT_INFRA\t%s" % x)
for x in exempt_pending: print("EXEMPT_PENDING\t%s" % x)
for g in gaps:           print("GAP\t%s" % g)
PY
)"
else
  # jq fallback: validates JSON + classifies each REQ.
  gate1_out="$(jq -r --arg infra "$INFRA_REQS" '
    ($infra | split(" ")) as $inf
    | "COUNT\t\(.requirements | length)",
      (.requirements[]
        | .id as $id
        | (.implementation_file != null and .implementation_file != "") as $hi
        | ((.test_file != null and .test_file != "") or ((.tests | length) > 0)) as $ht
        | if ($inf | index($id)) then "EXEMPT_INFRA\t\($id) (infrastructure)"
          elif .status == "pending" then "EXEMPT_PENDING\t\($id) (status=pending)"
          elif ($hi and $ht) then "OK\t\($id)"
          else "GAP\t\($id) missing pointers" end)
  ' "$REG" 2>&1 || echo "JSON_INVALID	jq could not parse $REG")"
fi

if grep -q "^JSON_INVALID" <<<"$gate1_out" || grep -q "^SCHEMA_INVALID" <<<"$gate1_out"; then
  echo "req.json is NOT valid:"
  grep -E "^(JSON_INVALID|SCHEMA_INVALID)" <<<"$gate1_out" | sed 's/^[^\t]*\t/  /'
  note_fail "Gate 1 (req.json integrity)"
else
  count="$(grep '^COUNT' <<<"$gate1_out" | cut -f2)"
  okn="$(grep -c '^OK' <<<"$gate1_out")"
  echo "valid JSON — $count requirements parsed; $okn fully traced (impl + test)."
  echo
  echo "  Exemptions (explicitly listed, NOT silently skipped):"
  if grep -q '^EXEMPT_INFRA' <<<"$gate1_out"; then
    grep '^EXEMPT_INFRA' <<<"$gate1_out" | cut -f2 | sed 's/^/    - /'
  fi
  if grep -q '^EXEMPT_PENDING' <<<"$gate1_out"; then
    grep '^EXEMPT_PENDING' <<<"$gate1_out" | cut -f2 | sed 's/^/    - /'
  fi
  if ! grep -qE '^EXEMPT_(INFRA|PENDING)' <<<"$gate1_out"; then
    echo "    (none)"
  fi
  echo
  if grep -q '^GAP' <<<"$gate1_out"; then
    echo "  REAL gaps (non-exempt REQ without impl+test pointer):"
    grep '^GAP' <<<"$gate1_out" | cut -f2 | sed 's/^/    - /'
    echo "RESULT: FAIL — non-exempt REQ(s) lack implementation/test pointers."
    note_fail "Gate 1 (req.json integrity)"
  else
    echo "RESULT: PASS — every non-exempt REQ has impl + test pointers."
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Gate 2 — annotation closure (delegate)
# ─────────────────────────────────────────────────────────────────────────────
banner "Gate 2/4: annotation closure (verify_req_annotations.sh)"

VERIFY="$SCRIPT_DIR/verify_req_annotations.sh"
if [[ ! -x "$VERIFY" && ! -f "$VERIFY" ]]; then
  echo "FATAL: verify_req_annotations.sh not found at $VERIFY" >&2
  note_fail "Gate 2 (annotation closure — script missing)"
else
  if bash "$VERIFY"; then
    echo "RESULT: PASS — annotations <-> registry consistent."
  else
    echo "RESULT: FAIL — annotation/registry mismatch (see above)."
    note_fail "Gate 2 (annotation closure)"
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Gate 3 — build + test
# ─────────────────────────────────────────────────────────────────────────────
banner "Gate 3/4: cargo build --workspace && cargo test --workspace"

if ! command -v cargo >/dev/null 2>&1; then
  echo "FATAL: cargo not on PATH" >&2
  note_fail "Gate 3 (build/test — cargo missing)"
else
  if cargo build --workspace --manifest-path "$ROOT/Cargo.toml"; then
    echo "-- build OK; running tests --"
    if cargo test --workspace --manifest-path "$ROOT/Cargo.toml"; then
      echo "RESULT: PASS — workspace builds and tests pass."
    else
      echo "RESULT: FAIL — cargo test --workspace failed."
      note_fail "Gate 3 (cargo test)"
    fi
  else
    echo "RESULT: FAIL — cargo build --workspace failed."
    note_fail "Gate 3 (cargo build)"
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Gate 4 — deviation-ledger approval status
# ─────────────────────────────────────────────────────────────────────────────
banner "Gate 4/4: deviation ledger approval (~/.fleet/deviations.jsonl)"

if [[ ! -f "$DEVIATIONS" ]]; then
  echo "No deviations file at $DEVIATIONS — nothing to approve. SKIP (not a failure)."
else
  # Each line is a JSON object; flag any with status == "pending".
  pending_lines=""
  if [[ "$JSON_TOOL" == "python3" ]]; then
    pending_lines="$(python3 - "$DEVIATIONS" <<'PY'
import json, sys
n = 0
with open(sys.argv[1]) as f:
    for ln in f:
        ln = ln.strip()
        if not ln:
            continue
        try:
            obj = json.loads(ln)
        except Exception:
            # A malformed deviation line is itself a fidelity problem.
            print("MALFORMED\t%s" % ln[:120])
            n += 1
            continue
        if obj.get("status") == "pending":
            print("PENDING\t%s" % obj.get("id", obj.get("req", "<no-id>")))
            n += 1
sys.exit(0)
PY
)"
  else
    # jq fallback: -e per line via slurp.
    pending_lines="$(jq -r 'select(.status=="pending") | "PENDING\t\(.id // .req // "<no-id>")"' "$DEVIATIONS" 2>&1 || echo "MALFORMED	jq parse error")"
  fi

  if [[ -z "$pending_lines" ]]; then
    echo "All deviation-ledger entries are resolved (no status=pending). RESULT: PASS."
  else
    echo "Unapproved / malformed deviation-ledger entries found:"
    sed 's/^[^\t]*\t/  - /' <<<"$pending_lines"
    echo "RESULT: FAIL — resolve/approve pending deviations before committing."
    note_fail "Gate 4 (deviation ledger)"
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Out-of-CI reminder — Auditor critical=0 (NOT counted toward exit code)
# ─────────────────────────────────────────────────────────────────────────────
banner "Out-of-CI: Auditor critical=0 (manual — NOT part of exit code)"
cat <<'EOF'
  REQ-045's acceptance includes an Auditor (P10) check requiring Auditor
  critical findings == 0. A full Auditor run needs an INDEPENDENT Claude
  session reasoning over the diff — this cannot run inside a headless CI
  runner, so it is NOT executed here and NOT faked.

  In CI this sub-check is DEGRADED to a human gate: a reviewer must run the
  Auditor (P10) out-of-band and confirm critical == 0 before merge.

  STATUS: manual / out-of-CI — excluded from this script's exit code by design.
EOF

# ─────────────────────────────────────────────────────────────────────────────
# Final verdict
# ─────────────────────────────────────────────────────────────────────────────
banner "spec-fidelity verdict"
if [[ "$overall" -eq 0 ]]; then
  echo "ALL IN-CI GATES PASSED ✓  (Auditor critical=0 remains a manual gate)"
else
  echo "SPEC-FIDELITY FAILED ✗  — failing gate(s):"
  for f in "${fail_summary[@]}"; do echo "  - $f"; done
fi
exit "$overall"
