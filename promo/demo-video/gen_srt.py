#!/usr/bin/env python3
"""marks.json ([{ms, sub}]) → subs.srt. Each cue runs until the next mark."""
import json, sys

marks = json.load(open(sys.argv[1] if len(sys.argv) > 1 else "marks.json"))
TAIL_MS = 4500  # duration of the last cue

def ts(ms):
    ms = max(0, int(ms))
    h = ms // 3600000; ms -= h * 3600000
    m = ms // 60000; ms -= m * 60000
    s = ms // 1000; ms -= s * 1000
    return f"{h:02d}:{m:02d}:{s:02d},{ms:03d}"

out = []
for i, mk in enumerate(marks):
    start = mk["ms"]
    end = marks[i + 1]["ms"] if i + 1 < len(marks) else start + TAIL_MS
    # keep a tiny gap so cues don't visually collide
    end = max(start + 900, end - 120)
    out.append(f"{i+1}\n{ts(start)} --> {ts(end)}\n{mk['sub']}\n")

open("subs.srt", "w").write("\n".join(out))
print(f"wrote subs.srt with {len(marks)} cues; last ends ~{ts(marks[-1]['ms']+TAIL_MS)}")
