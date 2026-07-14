#!/usr/bin/env bash
# Render the original BGM: MIDI -> fluidsynth(GM) -> normalized 66s mp3.
# Soundfont is a build input (not committed); fetched to a gitignored cache on demand.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cache="${TMPDIR:-/tmp}/claw-promo-sf"
sf="$cache/FluidR3Mono_GM.sf3"
mkdir -p "$cache"
if [ ! -f "$sf" ]; then
  echo "[bgm] fetching GM soundfont (MIT) ..."
  curl -sL --max-time 180 -o "$sf" \
    "https://github.com/musescore/MuseScore/raw/2.1/share/sound/FluidR3Mono_GM.sf3"
fi
wav="$here/bgm.wav"
fluidsynth -ni -g 0.9 -r 44100 -F "$wav" "$sf" "$here/bgm.mid" >/dev/null 2>&1
# trim/fade to exactly 66s, gentle master, encode mp3 for Remotion public/
out="$here/../public/audio/bgm.mp3"
ffmpeg -y -v error -i "$wav" \
  -af "atrim=0:66,afade=t=in:st=0:d=1.2,afade=t=out:st=63.5:d=2.5,loudnorm=I=-20:TP=-2:LRA=11,alimiter=limit=0.9" \
  -ar 44100 -b:a 192k "$out"
rm -f "$wav"
dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$out")
echo "[bgm] wrote $out  dur=${dur}s"
