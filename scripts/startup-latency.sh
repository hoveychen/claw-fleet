#!/usr/bin/env bash
# Break a new session's startup into legs, from evidence rather than guesses.
#
# For every `new_session: spawned` / `resume_session: spawned` line in the
# desktop debug log since SINCE (default: today), prints seconds after spawn at
# which:
#   transcript  the CLI wrote its first transcript record (boot + SessionStart hooks)
#   visible     the desktop first published a session list containing the id
#               (`[SPAWN-VISIBLE]`, desktop builds from 2026-10-04 on)
#   first-out   the first assistant record landed (model's first output)
#
# Usage: scripts/startup-latency.sh [SINCE] [LOG]
#   SINCE  "YYYY-MM-DD" or "YYYY-MM-DD HH:MM"   (default: today)
#   LOG    debug log path                       (default: ~/.fleet/claw-fleet-debug.log)
set -euo pipefail
SINCE="${1:-$(date +%F)}"
LOG="${2:-$HOME/.fleet/claw-fleet-debug.log}"
exec python3 - "$SINCE" "$LOG" <<'PY'
import glob, json, os, re, sys
from datetime import datetime, timezone

since, log = sys.argv[1], sys.argv[2]
spawn_re = re.compile(r"^\[(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)\] (new|resume)_session: spawned pid \d+ (?:for )?session ([0-9a-f-]{36})")
vis_re = re.compile(r"^\[[^\]]+\] \[SPAWN-VISIBLE\] ([0-9a-f-]{36}) \+(\d+)ms")

spawns, visible = [], {}
with open(log, "rb") as f:
    for raw in f:
        if not raw.startswith(b"[") or raw[1:1 + len(since)].decode("ascii", "replace") < since:
            continue
        line = raw.decode("utf-8", "replace")
        m = spawn_re.match(line)
        if m:
            spawns.append((m.group(1), m.group(2), m.group(3)))
            continue
        m = vis_re.match(line)
        if m:
            visible.setdefault(m.group(1), []).append(int(m.group(2)) / 1000)

def iso(ts):
    return datetime.fromisoformat(ts.replace("Z", "+00:00")).timestamp()

def legs(sid, t0):
    paths = glob.glob(os.path.expanduser(f"~/.claude/projects/*/{sid}.jsonl"))
    if not paths:
        return None, None
    first = asst = None
    with open(paths[0], "rb") as f:
        for raw in f:
            try:
                rec = json.loads(raw)
            except ValueError:
                continue
            ts = rec.get("timestamp")
            if not ts:
                continue
            t = iso(ts)
            if t < t0:  # an earlier turn of a resumed session
                continue
            if first is None:
                first = t - t0
            if rec.get("type") == "assistant":
                asst = t - t0
                break
    return first, asst

def fmt(v):
    return "      -" if v is None else f"{v:6.1f}s"

print(f"{'spawned':19}  kind    session   transcript  visible  first-out")
for ts, kind, sid in spawns:
    # The log stamps local time with one-second resolution.
    t0 = datetime.strptime(ts, "%Y-%m-%d %H:%M:%S").timestamp()
    first, asst = legs(sid, t0)
    vis = visible.get(sid, [None]).pop(0) if visible.get(sid) else None
    print(f"{ts}  {kind:6}  {sid[:8]}  {fmt(first)}   {fmt(vis)}   {fmt(asst)}")
PY
