#!/usr/bin/env python3
"""Burn subtitles WITHOUT libass/drawtext: render each cue to a PNG with
ImageMagick (white fill + thick black outline, readable on the light UI) and
overlay it onto the video with a timed `enable`.

Usage: burn_overlay.py <in.webm> <marks.json> <out.mp4>
"""
import json, subprocess, sys, os

IN = sys.argv[1] if len(sys.argv) > 1 else "out.webm"
MARKS = sys.argv[2] if len(sys.argv) > 2 else "marks.json"
OUT = sys.argv[3] if len(sys.argv) > 3 else "demo.mp4"
TAIL = 4.5

raw = open(MARKS).read().strip()
d = json.loads(raw)
if isinstance(d, str):
    d = json.loads(d)

W, H = 2560, 1440
CANVAS_W, CANVAS_H = 2360, 130
FONT = "Helvetica-Bold"

# 1) render each cue to a PNG
os.makedirs("subpng", exist_ok=True)
cues = []
for i, m in enumerate(d):
    start = m["ms"] / 1000.0
    end = (d[i + 1]["ms"] / 1000.0) if i + 1 < len(d) else start + TAIL
    end = max(start + 0.9, end - 0.12)
    png = f"subpng/s{i:02d}.png"
    txt = m["sub"]
    subprocess.run([
        "magick", "-size", f"{CANVAS_W}x{CANVAS_H}", "xc:none",
        "-gravity", "center", "-font", FONT, "-pointsize", "46",
        "-stroke", "black", "-strokewidth", "7", "-annotate", "0", txt,
        "-stroke", "none", "-fill", "white", "-annotate", "0", txt,
        png,
    ], check=True)
    cues.append((png, start, end))

# 2) build ffmpeg overlay chain
inputs = ["-i", IN]
for png, _, _ in cues:
    inputs += ["-i", png]

fc = []
prev = "[0:v]"
y = f"{H - CANVAS_H - 70}"
for idx, (_, s, e) in enumerate(cues):
    inp = f"[{idx + 1}:v]"
    out = f"[v{idx}]" if idx < len(cues) - 1 else "[vout]"
    fc.append(
        f"{prev}{inp}overlay=x=(W-w)/2:y={y}:enable='between(t,{s:.3f},{e:.3f})'{out}"
    )
    prev = f"[v{idx}]"

cmd = ["ffmpeg", "-y", *inputs, "-filter_complex", ";".join(fc),
       "-map", "[vout]", "-c:v", "libx264", "-preset", "slow", "-crf", "18",
       "-pix_fmt", "yuv420p", "-movflags", "+faststart", OUT]
print("cues:", len(cues))
r = subprocess.run(cmd, capture_output=True, text=True)
if r.returncode != 0:
    print("FFMPEG FAILED:\n", r.stderr[-1500:])
    sys.exit(1)
print("wrote", OUT)
