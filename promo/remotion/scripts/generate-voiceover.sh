#!/usr/bin/env bash
set -euo pipefail

scripts_file="${1:-voice-scripts.tsv}"
output_dir="${2:-public/audio/vo-2026}"
voice="${EDGE_TTS_VOICE:-en-US-BrianMultilingualNeural}"

mkdir -p "$output_dir"

while IFS=$'\t' read -r id line; do
  raw_file="$output_dir/.raw-$id.mp3"
  final_file="$output_dir/$id.mp3"
  edge-tts --voice "$voice" --rate=+5% --text "$line" --write-media "$raw_file"
  ffmpeg -nostdin -y -loglevel error -i "$raw_file" \
    -af "loudnorm=I=-16:TP=-1.5:LRA=11" -c:a libmp3lame -b:a 192k "$final_file"
  rm "$raw_file"
done < "$scripts_file"
