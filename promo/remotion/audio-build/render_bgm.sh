#!/usr/bin/env bash
# Render the original BGM and side-chain-duck it under the voiceover.
#   MIDI -> fluidsynth(GM) -> 66s music bed -> sidechaincompress(key=narration) -> bgm.mp3
# The narration "key" is the 11 VO clips placed at their scene offsets, so the music
# automatically dips while the captain speaks and lifts back up in the gaps.
# Soundfont is a build input (not committed); fetched to a gitignored cache on demand.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
aud="$here/../public/audio"
cache="${TMPDIR:-/tmp}/claw-promo-sf"
sf="$cache/FluidR3Mono_GM.sf3"
mkdir -p "$cache"
if [ ! -f "$sf" ]; then
  echo "[bgm] fetching GM soundfont (MIT) ..."
  curl -sL --max-time 180 -o "$sf" \
    "https://github.com/musescore/MuseScore/raw/2.1/share/sound/FluidR3Mono_GM.sf3"
fi

# 1) render the music to a wav
wav="$here/bgm.wav"
fluidsynth -ni -g 0.9 -r 44100 -F "$wav" "$sf" "$here/bgm.mid" >/dev/null 2>&1

# 2) voiceover clips in scene order + their start offset in ms (must match
#    ClawFleetPromo.tsx Series durations @30fps: 150,180*9,210).
VO=( vo-intro vo-tip1 vo-tip2 vo-tip3 vo-tip4 vo-tip5 vo-tip6 vo-tip7 vo-tip8 vo-tip9 vo-outro )
OFF=( 0        5000    11000   17000   23000   29000   35000   41000   47000   53000   59000 )

# 3) build the ffmpeg command: input 0 = music wav, inputs 1..11 = VO clips
inputs=( -i "$wav" )
for v in "${VO[@]}"; do inputs+=( -i "$aud/$v.mp3" ); done

# sidechaincompress lops ~2.8s off the tail of its output, so we run the whole
# duck 3s LONG (69s bed + 69s key) and hard-trim to 66s afterwards. Fades/limiter
# go AFTER the duck so their timing lands on the final 66s master.
# music bed: mastered, 69s (music itself is ~80s).
fc="[0:a]atrim=0:69,loudnorm=I=-20:TP=-2:LRA=11[bed];"
# narration key: delay each VO to its scene offset, mix (no renorm), pad to 69s.
keys=""
for i in "${!VO[@]}"; do
  n=$((i+1)); ms=${OFF[$i]}
  fc+="[$n:a]adelay=${ms}|${ms}[k$n];"
  keys+="[k$n]"
done
fc+="${keys}amix=inputs=${#VO[@]}:normalize=0,apad,atrim=0:69[key];"
# duck, then trim to the real 66s and apply fades + safety limiter
fc+="[bed][key]sidechaincompress=threshold=0.03:ratio=9:attack=12:release=380:makeup=2:detection=rms[duck];"
fc+="[duck]atrim=0:66,afade=t=in:st=0:d=1.2,afade=t=out:st=63.5:d=2.5,alimiter=limit=0.9[out]"

out="$aud/bgm.mp3"
ffmpeg -nostdin -y -v error "${inputs[@]}" -filter_complex "$fc" -map "[out]" -ar 44100 -b:a 192k "$out"
rm -f "$wav"
dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$out")
echo "[bgm] wrote $out  dur=${dur}s (side-chain ducked under narration)"
