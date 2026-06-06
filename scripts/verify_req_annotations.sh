#!/usr/bin/env bash
# verify_req_annotations.sh — [REQ-044][REQ-024][REQ-045 sub-check #2]
#
# Spec-fidelity annotation-consistency gate. Compares two sets:
#   A. CODE  = the [REQ-NNN] annotations actually present in the source tree
#   B. REG   = the REQ ids declared in registry/req.json
# and reports the symmetric difference:
#   - "orphan annotation": a [REQ-NNN] in code with no matching registry entry
#   - "orphan REQ":        a registry entry with no [REQ-NNN] anywhere in code
# Exit 1 if either set is non-empty (so CI / push gate fails); exit 0 when closed.
#
# Canonical annotation form is the BRACKETED, 3-digit token: [REQ-NNN].
# Bare "REQ-27" / "REQ-999" tokens inside string literals or test fixtures are
# deliberately NOT matched — only the [REQ-NNN] comment marker counts as a real
# traceability annotation. This is what keeps test-data ids (e.g. the
# "[DEVIATION] ... affects REQ-099" fixture strings in actions.rs) from being
# mistaken for implementation annotations.
#
# Some registry entries are pure registry/CI *infrastructure* (the registry and
# scripts this very task produces) and have no in-code [REQ-NNN] annotation by
# design. Those ids are listed in INFRA_REQS below and excluded from the
# "orphan REQ" failure set, but still printed so the report stays honest.

set -uo pipefail

# Resolve repo root from this script's location (scripts/ lives at repo root).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REG="$ROOT/registry/req.json"

SRC_DIRS=(
  "$ROOT/claw-fleet-task/src"
  "$ROOT/claw-fleet-core/src"
  "$ROOT/fleet-task/src"
)

# Registry ids that are infrastructure-only (no in-code annotation expected).
# These are the deliverables of the registry/CI work itself.
INFRA_REQS="REQ-023 REQ-024 REQ-044 REQ-045"

if [[ ! -f "$REG" ]]; then
  echo "FATAL: registry not found at $REG" >&2
  exit 2
fi

# --- Set A: REQ ids annotated in code (canonical bracketed 3-digit form) ------
code_ids="$(
  for d in "${SRC_DIRS[@]}"; do
    [[ -d "$d" ]] && grep -rhoE '\[REQ-[0-9]{3}\]' "$d" 2>/dev/null
  done | grep -oE 'REQ-[0-9]{3}' | sort -u
)"

# --- Set B: REQ ids declared in the registry -----------------------------------
# Prefer jq; fall back to python3; fall back to grep so the gate runs anywhere.
if command -v jq >/dev/null 2>&1; then
  reg_ids="$(jq -r '.requirements[].id' "$REG" | sort -u)"
elif command -v python3 >/dev/null 2>&1; then
  reg_ids="$(python3 -c 'import json,sys; [print(r["id"]) for r in json.load(open(sys.argv[1]))["requirements"]]' "$REG" | sort -u)"
else
  reg_ids="$(grep -oE '"REQ-[0-9]{3}"' "$REG" | tr -d '"' | sort -u)"
fi

# --- Diff ----------------------------------------------------------------------
# orphan annotations: in CODE but not in REGISTRY
orphan_annotations="$(comm -23 <(printf '%s\n' "$code_ids") <(printf '%s\n' "$reg_ids"))"
# orphan REQs (raw): in REGISTRY but not in CODE
orphan_reqs_raw="$(comm -13 <(printf '%s\n' "$code_ids") <(printf '%s\n' "$reg_ids"))"

# Split raw orphan REQs into infrastructure (expected) vs real gaps.
orphan_reqs_infra=""
orphan_reqs_real=""
for id in $orphan_reqs_raw; do
  if [[ " $INFRA_REQS " == *" $id "* ]]; then
    orphan_reqs_infra+="$id "
  else
    orphan_reqs_real+="$id "
  fi
done
orphan_reqs_infra="$(echo "$orphan_reqs_infra" | xargs -n1 2>/dev/null | sort -u)"
orphan_reqs_real="$(echo "$orphan_reqs_real" | xargs -n1 2>/dev/null | sort -u)"

# --- Report --------------------------------------------------------------------
echo "==== REQ annotation traceability check ===="
echo "code annotations : $(printf '%s\n' "$code_ids" | grep -c . ) ids"
echo "registry entries : $(printf '%s\n' "$reg_ids"  | grep -c . ) ids"
echo

echo "-- orphan annotations (in code, NOT in registry) --"
if [[ -z "$orphan_annotations" ]]; then echo "  (none)"; else printf '  %s\n' $orphan_annotations; fi
echo

echo "-- orphan REQs: infrastructure (expected, no in-code annotation) --"
if [[ -z "$orphan_reqs_infra" ]]; then echo "  (none)"; else printf '  %s\n' $orphan_reqs_infra; fi
echo

echo "-- orphan REQs: REAL gaps (registry entry with no [REQ-NNN] in code) --"
if [[ -z "$orphan_reqs_real" ]]; then echo "  (none)"; else printf '  %s\n' $orphan_reqs_real; fi
echo

fail=0
[[ -n "$orphan_annotations" ]] && fail=1
[[ -n "$orphan_reqs_real"    ]] && fail=1

if [[ "$fail" -eq 0 ]]; then
  echo "RESULT: PASS — code annotations and registry are consistent (infra REQs excluded by design)."
else
  echo "RESULT: FAIL — orphan annotations and/or real orphan REQs found above."
fi
exit "$fail"
