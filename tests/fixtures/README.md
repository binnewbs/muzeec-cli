# Test fixtures

Tiny audio files used by codec/gapless tests live here. Generate them with:

```sh
./scripts/gen-fixtures.sh      # requires ffmpeg
```

The script writes ~1 second, 440 Hz stereo tones in WAV/FLAC/MP3/Vorbis/Opus/AAC.
Total size stays under ~2 MB. Engine unit tests also synthesize WAV tones on the
fly, so the suite passes without this directory being populated.
