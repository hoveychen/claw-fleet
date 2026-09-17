#!/bin/sh
# Exit 0 once the account can actually serve a probe again.
#
# Asserts the fact (a real request succeeds), not the proxy (the wall clock),
# but gates on the clock first so we don't burn a request every poll while the
# reset is still hours away. The reset time reported by the 429 is 21:40 local.
set -u

RESET_HHMM=2140
CFG="$HOME/.guidance-probe/cfg"
WS="$HOME/.guidance-probe/tokws"

now=$(date +%H%M)
[ "$now" -ge "$RESET_HHMM" ] || exit 1

mkdir -p "$WS"
out=$(cd "$WS" && CLAUDE_CONFIG_DIR="$CFG" claude -p "Reply with exactly: OK" \
        --output-format json --model claude-haiku-4-5-20251001 2>&1)

case "$out" in
  *"session limit"*) exit 1 ;;
  *'"is_error":true'*) exit 1 ;;
esac

# `"subtype":"success"` alone is not enough — the CLI reports that field even
# for a failed turn (a not-logged-in run carries is_error:true AND
# subtype:success), so require the error flag to be explicitly false.
printf '%s\n' "$out" | grep -q '"is_error":false' || exit 1
exit 0
