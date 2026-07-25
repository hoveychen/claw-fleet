# Demo screencast (promo-mock-demo)

A no-voiceover, subtitled screen recording of the desktop app — the "raw mock
walkthrough" promo route (an alternative to the produced Remotion film in
`../remotion`). It shows a busy real board, dispatching a big task, the 5-hop
REST→gRPC handoff relay streaming one session at a time, and a feature tour.

## What's here

| file | role |
|------|------|
| `hero.js` | patchwright-cli `run-code` script: drives the app and records `out.webm`. Returns the measured subtitle timeline. |
| `burn_overlay.py` | Burns subtitles as timed PNG overlays (this box's ffmpeg has no libass/drawtext). |
| `gen_srt.py` | `marks.json` → `subs.srt` (for an external/sidecar subtitle track). |
| `marks.json` | The subtitle timeline captured from the last record (offsets are relative to screencast start). |
| `subs.srt` | Subtitle track. |
| `demo.mp4` | The finished 2560×1440 video with burned subtitles (~113s). |

## The mock behind it

Everything runs in the desktop app's `?mock&demo` mode (gated so it doesn't
touch the existing screenshot/footage tooling):
- `claw-fleet-desktop/app/mock/demoData.ts` — 37 real claude-fleet sessions
  (translated to English) as the board + a 5-hop relay on `aurora-platform`.
- `claw-fleet-desktop/app/mock/demoScripts.ts` — the per-hop screenplays.
- `claw-fleet-desktop/app/mock/tauri-mock.ts` — the relay streaming engine
  (`read_live_thinking` + `session-tail`).

## Reproduce

```bash
# 1. dev server (from claw-fleet-desktop/, worktree-friendly port)
pnpm install && node_modules/.bin/vite --port 1425 --strictPort

# 2. record (from a scratch cwd — patchwright-cli writes out.webm to its cwd)
patchwright-cli open
patchwright-cli run-code --filename hero.js   # ~113s; prints the marks timeline

# 3. subtitles + encode
python3 <(paste the marks output into marks.json)   # or reuse marks.json
python3 burn_overlay.py out.webm marks.json demo.mp4

# optional 4K upscale (the app layout fills 1440p, not true 4K; this is a
# lanczos upscale, not native 4K):
ffmpeg -i demo.mp4 -vf scale=3840:2160:flags=lanczos -c:v libx264 -crf 18 \
  -pix_fmt yuv420p -movflags +faststart demo.4k.mp4
```

Resolution note: true-4K viewport (3840×2160) leaves the app sparse (fixed
max-width, left-aligned), so the record runs at 2560×1440 where the layout
fills edge to edge. Upscale to 2160p in post if a 4K file is required.
