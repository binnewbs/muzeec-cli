#!/usr/bin/env bash
# Generate tiny CC0-style audio fixtures for engine tests. Requires ffmpeg.
# Outputs are intentionally small (< 2 MB total).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/tests/fixtures"
mkdir -p "$OUT"

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "ffmpeg is required to generate fixtures" >&2
  exit 1
fi

# 1 second 440 Hz sine at 44.1 kHz, stereo.
gen() {
  local out="$1"; shift
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" \
    -ac 2 "$@" "$OUT/$out"
}

gen tone.wav
gen tone.flac -c:a flac
gen tone.mp3  -c:a libmp3lame -b:a 128k
gen tone.ogg  -c:a libvorbis -q:a 4
if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libopus; then
  gen tone.opus -c:a libopus -b:a 96k
fi
if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q aac; then
  gen tone.m4a -c:a aac -b:a 128k
fi

echo "fixtures written to $OUT"
ls -lh "$OUT"
