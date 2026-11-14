# Architecture & decisions log

This file records the choices behind the implementation, including where the
implementation deliberately differs from the initial plan.

## Process model

```
CLI / TUI ──NDJSON over Unix socket──► daemon ──► engine threads ──► cpal
                                          │
                                          ├─ SQLite library (single writer)
                                          ├─ MPRIS2 (zbus/mpris-server)
                                          └─ notifications
```

The daemon is the only process that touches the database or the engine. The
socket lives at `$XDG_RUNTIME_DIR/muzeec-cli/sock`, is chmod `0600`, and the
server checks `SO_PEERCRED` against its own uid before serving a connection.

## Audio engine

- **Decode**: `symphonia` 0.6 with a custom `CodecRegistry` that registers
  `symphonia_opus::OpusDecoder` at `Tier::Fallback`. Gapless trimming is enabled
  through `AudioDecoderOptions` (default on).
- **Resampling**: decoded audio is converted to the device rate/channel layout
  with a small stateful linear interpolator (`engine/resample.rs`). This keeps
  the decoder streaming and allocation-free per frame. The plan suggested
  `rubato`; rubato 5 introduced an `audioadapter`-based API that made the
  streaming path disproportionately complex for v1, so the built-in
  interpolator is used and a higher-quality offline resampler can be added
  behind the same interface later.
- **Threading**:
  - *audio thread* owns the `cpal` stream for its lifetime;
  - *worker thread* owns the decoder(s) and writes to an `rtrb` SPSC ring;
  - the *callback* only pops samples, applies gain/limiter/EQ and advances a
    position atomic — no locks, allocation, or I/O;
  - *control/daemon threads* mutate queue state and publish snapshots through
    `arc-swap`.
- **Gapless**: the daemon preloads the next track with `SetNext`. When the
  current decoder is exhausted the worker immediately starts the preloaded
  decoder into the same ring, so there is no silence and no device reopen.
- **Crossfade**: when configured, the worker reads the tail of the current
  track and the head of the next, mixes them with equal-power `cos/sin`
  gains (`engine/mixer.rs`), and switches decoders at the boundary.
- **Fallback**: if no audio device can be opened, the engine falls back to a
  `NullSink` (real-time clock, silent) so library, search, and playlist
  management still work in headless environments. `doctor` reports devices.

## Library

- SQLite in WAL mode with FTS5 (`unicode61 remove_diacritics 2`, prefix
  indexes) and sync triggers.
- The `albums` table is a derived materialized view refreshed after scans.
- Scanning is incremental by `(size, mtime)`, parses tags in parallel with
  `rayon`, persists in 500-row batches, and never aborts on a corrupt file
  (records `error` instead).
- Artwork is content-addressed by BLAKE3 with resized thumbnails and an LRU
  size cap.

## Search

Free text is converted to safe FTS5 prefix terms; filter tokens
(`artist:`, `album:`, `genre:`, `year:`, `is:lossless`, `dur:>5m`, `rated:>=4`)
become SQL predicates. Ranking uses BM25 with field boosts:
title 10, artist 5, album 3, albumartist 3, genre 2, filename 1.

## IPC protocol

Newline-delimited JSON. Requests `{id, cmd, params}`; responses
`{id, ok, data|error}`; events `{event, data}`. Version handshake via `hello`.
`subscribe` attaches a client to the event stream (`track_changed`,
`track_ended`, `stopped`, `error`).

## MPRIS

`mpris-server`'s ready-made `Player` runs on a dedicated current-thread tokio
runtime (`LocalSet`). Incoming methods dispatch through the same daemon
command router as the CLI; state is pushed every 500 ms (playback status,
position, loop/shuffle, volume) and metadata on track change, including
`file://` art URLs from the cache.

## Deviations from PLAN.md

- Built-in streaming resampler instead of `rubato` (the rubato 5 API is
  interposer-oriented; see above).
- The TUI currently renders text and progress; album art is available via
  `muzeec-cli art --render` and MPRIS `artUrl`, and the ratatui-image widget is
  a follow-up. Fuzzy reranking uses SQL prefiltering for v1.
- EQ is implemented in the DSP path but remains config-only/off by default.
