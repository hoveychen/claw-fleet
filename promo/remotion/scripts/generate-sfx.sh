#!/usr/bin/env bash
set -euo pipefail

output_dir="${1:-public/audio/sfx}"
mkdir -p "$output_dir"

ffmpeg -y -loglevel error \
  -f lavfi -i "aevalsrc=0.20*sin(2*PI*920*t)*exp(-38*t)+0.08*sin(2*PI*1540*t)*exp(-48*t):s=48000:d=0.14" \
  -c:a pcm_s16le "$output_dir/click.wav"

ffmpeg -y -loglevel error \
  -f lavfi -i "anoisesrc=color=pink:duration=0.48:amplitude=0.32:sample_rate=48000" \
  -af "highpass=f=280,lowpass=f=5200,afade=t=in:st=0:d=0.08,afade=t=out:st=0.18:d=0.30,volume=1.6" \
  -c:a pcm_s16le "$output_dir/whoosh.wav"

ffmpeg -y -loglevel error \
  -f lavfi -i "aevalsrc=0.18*sin(2*PI*330*t)*exp(-7*t)+0.14*sin(2*PI*220*t)*exp(-5*t):s=48000:d=0.52" \
  -af "afade=t=out:st=0.30:d=0.22" -c:a pcm_s16le "$output_dir/alert.wav"

ffmpeg -y -loglevel error \
  -f lavfi -i "sine=frequency=660:duration=0.22:sample_rate=48000" \
  -f lavfi -i "sine=frequency=990:duration=0.34:sample_rate=48000" \
  -filter_complex "[0:a]volume=2.2,afade=t=out:st=0.08:d=0.14[a0];[1:a]volume=1.8,adelay=110|110,afade=t=out:st=0.20:d=0.14[a1];[a0][a1]amix=inputs=2:duration=longest" \
  -c:a pcm_s16le "$output_dir/success.wav"
