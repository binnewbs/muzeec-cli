# muzeec-cli — Plan

A terminal music player in Rust with a real audio engine, a full library, and desktop
integration. Ships as an Arch package (`.pkg.tar.zst`); the package is built locally and
**never installed by the tooling** — you run `sudo pacman -U` yourself.

- Working name: **muzeec-cli** — package, main binary and config dir; daemon binary `muzeec-cli-daemon`
- Target: CachyOS / Arch x86_64 (also builds on aarch64)
- License: MIT
- Language: Rust (edition 2024, MSRV 1.85, built with system stable — currently 1.100)

---

## 1. Product goals

A player that is pleasant to *live in* from a terminal, not a toy:

1. **Correct audio** — sample-accurate gapless playback, equal-power crossfade, click-free
   seek/pause/volume, ReplayGain, optional EQ.
2. **Fast library** — incremental scan, SQLite + FTS5 search, fuzzy TUI filtering,
   metadata and album art for local files.
3. **Full queue/playlist control** — editable queue, history, repeat/shuffle, M3U import/export,
   persistent playlists, similarity "mix" (auto-DJ).
4. **First-class desktop citizen** — MPRIS2 (media keys, `playerctl`, KDE/GNOME widgets),
   notifications, `.desktop` entry, icon, systemd user unit (shipped disabled).
5. **Scriptable** — every action has a CLI/JSON equivalent; a Waybar/Polybar status mode.
6. **Clean packaging** — one `makepkg` run → `dist/muzeec-cli-<ver>-<rel>-x86_64.pkg.tar.zst`.

### Feature map

| Requested | Where it lives |
|---|---|
| Gapless playback | §5.3 engine, deck handoff at exact frame boundary |
| Crossfade | §5.4 dual-deck equal-power mixer, 0–12 s |
| Loop | §6.4 off / one / all, per-queue, MPRIS `LoopStatus` |
| Queue | §6.1 ordered queue, up-next, history, move/remove/dedupe, persisted |
| Mix | §6.5 similarity-weighted auto-DJ / endless radio |
| Playlist editing | §6.2 DB playlists, M3U8 import/export, TUI + CLI editing |
| Library explorer | §7 scan + §8 TUI browser (artists/albums/tracks/genres) |
| Search | §6.6 FTS5 + filters, nucleo fuzzy in TUI |
| Album art | §9 ratatui-image (kitty/sixel/iterm2/halfblocks), MPRIS `artUrl` |
| Desktop integration | §10 MPRIS2, notifications, media keys, `.desktop`, service unit |
| Everything else | ReplayGain, EQ, spectrum visualizer, ratings, resume, themes, completions, man pages |

### Non-goals for v1 (backlog)

Internet radio/streaming, podcasts/audiobooks, Subsonic/Jellyfin, cloud sync,
Last.fm/ListenBrainz scrobbling, gapless DSP plugins, mobile remote. Architecture leaves
room for them but v1 stays local-files-only.

---

## 2. Why pure Rust (and not libmpv)

libmpv is installed and would give codecs for free, but:

- gapless/crossfade/seek become black boxes; sample-accurate behavior and a real spectrum
  tap are hard to guarantee;
- it adds a large runtime dependency and IPC to the package;
- the visualizer needs the post-mix PCM anyway.

**Decision: `symphonia` (decode) + `cpal` (output) + own mixer.** Fallback plan if a codec
gap blocks v1: add a `libmpv` backend behind a cargo feature (engine behind an `AudioSink`
trait makes that swap contained). Phase 0 must prove Opus/AAC/ALAC coverage.

---

## 3. Architecture

```
clients
  ├─ muzeec-cli (CLI)
  └─ muzeec-cli (TUI, ratatui)
        │
        │  newline-delimited JSON over
        │  $XDG_RUNTIME_DIR/muzeec-cli/sock
┌───────▼─────────────────────────────────────────────────────────┐
│ muzeec-cli-daemon (tokio)                                       │
│  ├─ IPC server + command router (single control thread)         │
│  ├─ MPRIS2 (zbus), notifications (notify-rust)                  │
│  ├─ library scanner (rayon) + file watcher (notify)             │
│  └─ Player facade ──► engine threads ──► cpal output stream     │
└─────────────────────────────────────────────────────────────────┘
                       │            │
              ┌────────▼───┐   ┌────▼────────┐
              │ decoder A  │   │ decoder B   │  (gapless prime / crossfade)
              └────────┬───┘   └────┬────────┘
                       │ rtrb ring buffers (lock-free)
                 ┌─────▼────────────▼─────┐
                 │ realtime mixer callback│ → device
                 │ gains, xfade, limiter  │ → FFT tap → TUI visualizer
                 └────────────────────────┘
```

Threads and ownership rules:

- **Control thread** owns `Player` state (queue, modes); all mutations serialized here.
- **Decoder threads** (1–2) decode → resample to engine rate → f32 interleaved → `rtrb`.
  They also expose exact frame counts and gapless trim info.
- **Mixer runs inside the cpal callback** — no allocation, no locks, no I/O, no `unwrap`.
  Parameters arrive via atomics / lock-free channels; output is preallocated.
- **Snapshots** for IPC/MPRIS/TUI are published with `arc-swap` (wait-free reads).
- **Never panic in the audio path**; decoder failures emit events and auto-skip (3 strikes
  → pause and notify).

### Workspace layout

```
muzeec-cli/
├── Cargo.toml                # workspace
├── Cargo.lock                # committed
├── rust-toolchain.toml       # channel = "stable"
├── rustfmt.toml
├── clippy.toml
├── justfile                  # dev: run/check/test/tui/pkg
├── README.md
├── LICENSE
├── crates/
│   ├── muzeec-cli-core/          # all logic, no UI
│   │   └── src/
│   │       ├── config.rs
│   │       ├── paths.rs      # XDG config/data/cache/state/socket
│   │       ├── db/{mod,schema,migrations,queries}.rs
│   │       ├── library/{scan,tags,art,watch}.rs
│   │       ├── engine/{mod,player,decode,resample,mixer,dsp,output,gapless}.rs
│   │       ├── queue.rs
│   │       ├── playlist.rs
│   │       ├── search.rs
│   │       ├── mix.rs
│   │       ├── replaygain.rs
│   │       └── ipc/{proto,server,client}.rs
│   ├── muzeec-cli-tui/           # ratatui app, no engine access
│   │   └── src/{app,keys,ui/*,art,visualizer,theme,command}.rs
│   ├── muzeec-cli/               # bin: clap CLI + launches TUI
│   └── muzeec-cli-daemon/        # bin: engine + IPC + MPRIS + notifications
├── assets/
│   ├── muzeec-cli.svg
│   ├── muzeec-cli.desktop
│   └── muzeec-cli-daemon.service
├── packaging/arch/PKGBUILD
├── scripts/{build-pkg.sh,gen-fixtures.sh,dev-daemon.sh}
├── tests/fixtures/           # tiny CC0-generated audio files
└── docs/{CONFIG.md,KEYBINDS.md,ARCHITECTURE.md,CONTRIBUTING.md}
```

### Dependencies (pin exact versions in Phase 0)

| Area | Crates (stable version found 2026-11) |
|---|---|
| Decode | `symphonia` 0.6.1 (`mp3`, `aac`, `alac`, `flac`, `vorbis`, `pcm`, `adpcm`, `ogg`, `isomp4`, `mkv`, `wav`, `aiff`, `opt-simd`), `symphonia-opus` 0.1.2 |
| Output | `cpal` 0.18.2 (ALSA default; build features `pipewire`, `pulseaudio` selectable), `rubato` 5.1, `rtrb` 0.4 |
| DSP | `ebur128` 0.1, `rustfft` 6.4 |
| Metadata | `lofty` 0.25, `walkdir`, `rayon`, `notify` (file watch), `image` 0.25 |
| Data | `rusqlite` 0.42 (system SQLite; FTS5 verified, `bundled` fallback), `serde`, `serde_json`, `toml`, `blake3` |
| UI | `ratatui` 0.30, `crossterm` (version matching ratatui), `ratatui-image` (stable 11.x or v12 after spike), `nucleo` 0.5 |
| Daemon | `zbus` 6.3 + `mpris-server` 0.11 (compat spike), `tokio`, `notify-rust` 4.19, `crossbeam-channel`, `arc-swap`, `parking_lot`, `rand` |
| CLI/ops | `clap` 4.6 (`derive`, `wrap_help`), `clap_complete`, `clap_mangen`, `tracing`, `tracing-subscriber`, `thiserror`, `color-eyre`, `directories`, `humantime` |

Pin every crate in `Cargo.toml` to an exact version and commit `Cargo.lock`; the Arch
package builds `--locked --frozen`.

---

## 4. CLI surface

`muzeec-cli` auto-spawns `muzeec-cli-daemon` if the socket is absent (unless `--no-daemon`).

```
muzeec-cli                          # open TUI (daemon starts if needed)
muzeec-cli tui [--theme NAME]
muzeec-cli play [QUERY|PATH...] [--replace|--next|--now] [--json]
muzeec-cli pause | resume | toggle | stop
muzeec-cli next | prev | seek <±DUR|PERCENT> | volume <0-100|+N|-N> | mute
muzeec-cli status [--json | --waybar | --short]
muzeec-cli queue [list|add Q|play-next Q|remove ID|move ID POS|clear|shuffle|save NAME]
muzeec-cli playlist [list|show NAME|create NAME|add NAME Q|remove NAME ID|move NAME ID POS
                 |rename OLD NEW|delete NAME|import FILE|export NAME FILE|play NAME]
muzeec-cli library [scan|rescan|stats|artists|albums|tracks|genres] [--json]
muzeec-cli search <QUERY> [--limit N] [--json]     # supports artist:, album:, year:, genre:, is:lossless
muzeec-cli mix [--seed QUERY|--from-now] [--hours H] [--replace]
muzeec-cli loop [off|one|all] | shuffle [on|off]
muzeec-cli art [--path|--render|--save FILE] [--size 40x20]
muzeec-cli import <M3U> [--replace] | muzeec-cli export <FILE>
muzeec-cli daemon [--foreground] | muzeec-cli doctor
muzeec-cli completions <bash|zsh|fish|elvish|powershell>
muzeec-cli config [path|edit|dump]
```

- `muzeec-cli doctor` checks: audio device open, engine rate, codec support matrix, SQLite FTS5,
  D-Bus/MPRIS name availability, notification daemon, art protocol, library writability.
- All machine-readable commands support `--json` with stable schemas.
- Exit codes: `0` ok, `1` runtime error, `2` usage, `3` daemon unreachable, `4` not found.
- Respects `NO_COLOR`, `CLICOLOR`, `--no-config`, `--config PATH`.

### IPC protocol

Unix stream socket at `$XDG_RUNTIME_DIR/muzeec-cli/sock`, mode `0600`, peer-uid checked.
Newline-delimited JSON, request/response + event subscription:

```json
{"id":7,"cmd":"player.play","params":{"tracks":[42,43],"mode":"replace"}}
{"id":7,"ok":true,"data":{"queue_len":2}}
{"event":"track_changed","data":{"id":42,"title":"…","artist":"…","art":"/cache/…jpg"}}
```

Commands: `status`, `player.*`, `queue.*`, `playlist.*`, `library.*`, `search`, `mix`,
`art.get`, `output.devices`, `subscribe`. The TUI is just another client — it never touches
the DB or engine directly (one writer, no lock contention).

---

## 5. Audio engine

### 5.1 Pipeline

```
file → symphonia demux/decode (gapless trimming) → rubato resample → per-deck gain
     → equal-power crossfade → ReplayGain + volume (perceptual) → 10-band EQ (opt)
     → soft limiter (-1 dBFS) → f32 output callback → device
                                              └→ FFT tap (visualizer)
```

- Engine rate = device default (usually 48 kHz), configurable. All tracks are resampled to
  it so tracks with mixed rates stay gapless.
- Decode format f32; channel layout normalized to device channels (mono→stereo, >2 mixed
  down or passed through when device supports it).
- Buffer: 512 frames (~10.7 ms @48k) target; configurable `buffer_ms` 10–100.
- Volume is perceptual (cubic curve) 0–100 → 0.0–1.0, mute independent.

### 5.2 Seeking

Sample-accurate: control thread computes target frame from duration, restarts the decoder
at that frame, applies a 5 ms fade-out/fade-in around the splice to prevent clicks.
`seek ±5s`, `seek 50%`, MPRIS `Seek` (μs) and `SetPosition` supported.

### 5.3 Gapless (the hard part)

- Enable symphonia `FormatOptions::enable_gapless`; use `n_frames` + encoder delay/padding
  so MP3 (LAME/Xing), AAC (iTunSMPB) and Opus pre-skip are trimmed exactly.
- One deck keeps playing; when the remaining frames ≤ prime threshold (~2 s), the control
  thread starts the *next* decoder and pre-fills its ring.
- At the exact last output frame of track N, the mixer switches its source pointer to
  track N+1. No drain, no silence, no re-open of the device.
- Track boundary metadata and ReplayGain changes are applied on the same frame.
- Unknown-length/streamy files (rare, local) fall back to duration-from-estimate.

### 5.4 Crossfade

- `crossfade_secs` 0–12 (default 0 ⇒ pure gapless).
- When enabled, deck B starts `crossfade_secs` before deck A ends; both mix with
  equal-power curves (`cos/sin` on a quarter wave) so perceived loudness stays flat.
- Manual skip uses a short 30 ms fade to avoid clicks; `stop` fades 150 ms then closes.
- Crossfade only on auto-advance; explicit `play --now` is a short fade.
- Config option `crossfade_on_album_change_only` for people who want gapless albums but
  crossfaded playlists.

### 5.5 Loudness

- Read `REPLAYGAIN_TRACK_GAIN/PEAK`, `REPLAYGAIN_ALBUM_GAIN/PEAK`, and `R128_*` tags via lofty.
- Missing values: `muzeec-cli library scan --analyze` computes EBU R128 integrated loudness and
  true peak with `ebur128`, stored in DB (target −18 LUFS, RG2 convention).
- Modes: `off | track | album`. Gain applied before the limiter; true-peak clamp prevents
  clipping.

### 5.6 EQ and DSP (v1.1, behind config)

10-band RBJ peaking biquads (31 Hz…16 kHz, Q≈1.41), preamp, presets
(flat/rock/jazz/bass/vocal), persisted per output device. State-space biquads, f64
coefficients, f32 processing. Off by default; no extra UI until stable.

### 5.7 Output device handling

- Enumerate cpal devices; `[playback] device = "default"` follows the system default and
  handles hot-swap (device removed → pause + notify + retry on new default).
- `muzeec-cli doctor` and `muzeec-cli status` show backend/device/rate/buffer.
- Backends: ALSA by default (works via `pipewire-alsa`/`pulseaudio-alsa`); cargo features
  `pipewire` and `pulseaudio` for native paths. Phase 0 picks the default based on real
  device tests on this machine, and the others stay opt-in.

### 5.8 Engine testability

`AudioSink` trait with three implementations: `CpalSink`, `FileSink` (WAV, for tests /
`--record`), `NullSink` (deterministic clock). Gapless/crossfade/seek tests run headless
at 48 kHz and inspect output samples.

---

## 6. Library, search, queue, playlists, mix

### 6.1 Queue

- Ordered `VecDeque<QueueItem>` with stable IDs, current index, "up next" view, history stack
  for `prev`.
- Operations: append, play-next, insert, remove, move, dedupe, clear, jump, shuffle
  (Fisher–Yates with seed; keeps current track on top).
- End-of-queue behavior: `loop off` stops; `loop all` wraps; `mix` extends automatically;
  shuffle re-shuffles when exhausted.
- Persisted in `$XDG_STATE_HOME/muzeec-cli/state.json` on every mutation (debounced 500 ms) and
  on shutdown; restored on daemon start (`resume_on_start`).

### 6.2 Playlists

- Named playlists in SQLite with ordered items, created/updated timestamps.
- Full editing from TUI (`a` add, `d` remove, `J/K` move, rename, delete) and CLI.
- M3U/M3U8 import/export with relative-path resolution, `#EXTINF` metadata, duplicate
  handling, and missing-file reporting.
- Optional smart playlists (saved query: genre/year/rating/last-played) in v1.1.

### 6.3 Library scanning

- `walkdir` over `music_dirs`, skipping hidden dirs and optionally symlinks.
- Tags via lofty (title, artist, albumartist, album, track/disc, year, genre, BPM, composer,
  comment, ReplayGain, embedded pictures).
- Fallbacks from filename patterns (`Artist - Album - 01 - Title`) when tags are missing.
- Incremental: compare `(mtime, size)`; unchanged files are skipped. Parallel parse with
  rayon, batched transactions of 500 rows, SQLite WAL.
- `notify` watcher (debounced 2 s) for adds/removes/renames while running; `SIGHUP` rescans.
- Corrupt/unreadable files are recorded with an error state, never abort the scan.
- Scan progress streamed to clients as events (`scanned/total`).

### 6.4 Modes

- Loop: `off`, `one`, `all` (MPRIS `LoopStatus` ↔ `None/Track/Playlist`).
- Shuffle: on/off, independent of loop, MPRIS `Shuffle`.
- History and play counts feed `mix` and smart shuffle; 0–5 user rating.

### 6.5 Mix (auto-DJ)

`muzeec-cli mix` builds a queue continuation that sounds intentional:

- Seed: current track or a query/path.
- Candidate features: artist, albumartist, genre tags, decade, BPM, loudness/energy,
  rating, play-count recency, co-occurrence in existing playlists.
- Score = weighted sum (artist 0.20, albumartist 0.10, genre 0.20, BPM closeness 0.15,
  energy closeness 0.15, rating 0.10, recency penalty 0.10), then weighted sampling from
  the top ~50 with low temperature so it is not deterministic.
- Hard filters: no repeats in the last N tracks (configurable), same-track exclusion.
- Endless mode: when 3 tracks remain, fetch the next candidate window and append.
- `muzeec-cli mix --hours 2` pre-fills a finite mix; `--replace` swaps the queue.

### 6.6 Search

- SQLite FTS5 over `title, artist, album, albumartist, genre, filename`
  (`unicode61 remove_diacritics 2`, prefix indexes).
- Ranking: BM25 with field boosts (title > artist > album > genre) and exact-prefix bonus.
- Filters parsed from query: `artist:`, `album:`, `genre:`, `year:`, `is:lossless`,
  `is:instrumental`, `dur:>5m`, `rated:>=4`.
- CLI search: FTS + rank, `--limit`, `--json`.
- TUI search: FTS prefilter (top 500) → nucleo fuzzy rerank for instant feedback;
  background full-library fuzzy fallback if fewer than 20 hits.
- Performance target: < 20 ms for 50k tracks (warm), < 150 ms cold.

---

## 7. Database

Location `$XDG_DATA_HOME/muzeec-cli/library.db` (WAL, `synchronous=NORMAL`, foreign keys on).

```sql
tracks(
  id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, filename TEXT,
  title, artist, albumartist, album, genre, composer, comment,
  track_no, disc_no, year, duration_ms, sample_rate, channels, bitrate, codec,
  bpm, lufs, true_peak, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak,
  size, mtime, art_hash, added_at, last_seen_at, error
);
albums(id, name, albumartist, year, art_hash, track_count, duration_ms);  -- derived, refreshed
playlists(id, name UNIQUE, created_at, updated_at);
playlist_items(playlist_id, position, track_id, PRIMARY KEY(playlist_id, position));
queue_state(id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT, updated_at);
history(id, track_id, played_at, ms_played, completed);
ratings(track_id PRIMARY KEY, stars, updated_at);
tracks_fts USING fts5(title, artist, album, albumartist, genre, filename,
                      content='tracks', content_rowid='id');
meta(key PRIMARY KEY, value);   -- schema_version is PRAGMA user_version
```

- Migrations: ordered `Vec<&str>` applied in a transaction, bumping `PRAGMA user_version`.
- `albums` is a materialized view refreshed after scans; avoids expensive GROUP BY on every
  browse. Refresh is incremental by album key.
- Art cache: `$XDG_CACHE_HOME/muzeec-cli/art/<blake3>.jpg` + size-capped LRU (default 512 MB).
- Apple Music/MusicBrainz lookups are out of scope; local embedded art and
  `cover/folder/front.*` sidecars only.

---

## 8. TUI (ratatui)

```
┌ muzeec-cli ────────────────────────────── ♪  68%  ▁▂▄▆█▆▄▂  loop:all  shuffle:on ┐
│ Library           │ Tracks — Kind of Blue        │ Now Playing              │
│  Artists          │  1  So What          9:22    │   ┌──────────────────┐   │
│  Albums           │  2  Freddie Freeloader 9:46  │   │                  │   │
│  Tracks           │  3  Blue in Green    5:37  ▶ │   │    album art     │   │
│  Genres           │  4  All Blues        11:33   │   │                  │   │
│  Playlists        │  5  Flamenco Sketches 9:25   │   └──────────────────┘   │
│  ─────────────    │                              │  Miles Davis             │
│  Queue (12)       │                              │  Kind of Blue (1959)     │
│  Mix              │                              │  FLAC · 44.1k · 16-bit   │
├───────────────────┴──────────────────────────────┴──────────────────────────┤
│ ▶ So What   ─────────────●────────────────────────────  2:41 / 9:22         │
│ / search…                     : command                    ? help  q quit    │
└─────────────────────────────────────────────────────────────────────────────┘
```

- Views: **Library** (Artists/Albums/Tracks/Genres/Playlists/Queue/Mix), **Now Playing**
  (art, metadata, progress, spectrum/oscilloscope), **Search overlay**, **Help**,
  **Inspector** (full tags, path, format, gain).
- Command bar with `:` (e.g. `:mix`, `:loop one`, `:save roadtrip`, `:eq bass`)
  plus `/` incremental search. Tab-completion with descriptions.
- Artwork: `ratatui-image` protocol autodetect (kitty → sixel → iterm2 → halfblocks),
  cached thumbnails resized to pane; `off` for low-bandwidth/SSH.
- Visualizer: 48 log-spaced bands, Hann window, 2048-point FFT on the post-mix tap,
  smoothing + peak-hold; modes `bars | osc | off`; 30 fps capped, only redraws when dirty.
- 3 built-in themes + user TOML theme overrides; respects `NO_COLOR` and 16-color terms.
- Mouse: click select, double-click play, wheel scroll, drag progress bar, right-click menu.
- Rendering: immediate-mode ratatui, dirty-flag loop with 250 ms tick + event wake; no busy
  polling. Target < 5% CPU idle, < 100 ms startup with warm DB.

### Keybindings (defaults, `~/.config/muzeec-cli/keys.toml` overridable)

| Key | Action |
|---|---|
| `j/k`, `↑/↓` | move selection |
| `h/l`, `←/→` | focus panes |
| `Enter` | play selection / open |
| `a` | add to queue (enqueue) |
| `A` | play next |
| `Space`, `p` | play/pause |
| `s` | stop |
| `n/N` | next / previous |
| `[`/`]` | seek −/+5 s |
| `1`…`9` | seek to percent |
| `+/-` | volume ±5 |
| `m` | mute |
| `r` | shuffle toggle |
| `R` | loop off → all → one |
| `x` | crossfade toggle (configured length) |
| `v` | visualizer mode |
| `i` | inspector |
| `u` / `U` | queue / history |
| `d` | remove from queue/playlist |
| `J/K` | move item down/up |
| `0`…`5` | set rating |
| `/` | search |
| `:` | command bar |
| `?` | help |
| `q` | quit TUI (daemon keeps playing) |

---

## 9. Album art

1. Embedded pictures via lofty (front cover preferred, then any).
2. Sidecar files: `cover.jpg/jpeg/png`, `folder.*`, `front.*`, `album.*` (case-insensitive).
3. Cache keyed by blake3 of the image bytes, deduplicated across albums; resized thumbnails
   (`480px`, `120px`) generated with `image`.
4. TUI rendering through `ratatui-image` (kitty/sixel/iterm2/halfblocks).
5. MPRIS `mpris:artUrl` → `file://` cached path (desktop notification/widgets show it).
6. CLI: `muzeec-cli art --path` (scripting), `--render` (terminal), `--save out.png`.

---

## 10. Desktop integration

- **MPRIS2** (`org.mpris.MediaPlayer2.muzeec-cli`): `Identity`, `DesktopEntry=muzeec-cli`,
  `PlaybackStatus`, full metadata (title, artist, album, artUrl, length, trackid, url),
  `Position`, `Volume`, `LoopStatus`, `Shuffle`, `Can*`; methods
  `Play/Pause/PlayPause/Stop/Next/Previous/Seek/SetPosition/OpenUri`; periodic
  `PropertiesChanged` for Position (250 ms while playing).
- Media keys work through the desktop's MPRIS binding (KDE/GNOME/Hyprland).
  No global key grabbing (Wayland-unfriendly); MPRIS is the contract.
- **Notifications** on track change (optional) and errors, with album art hint; coalesced
  and suppressed for rapid skips. Config `[integrations] notifications`.
- **Desktop entry** `muzeec-cli.desktop` (`Terminal=true`, `Exec=muzeec-cli tui`,
  `Categories=AudioVideo;Audio;Player;`) + hicolor SVG icon.
- **systemd user unit** `muzeec-cli-daemon.service` (shipped, not enabled) for people who want
  the daemon resident at login; normal use auto-spawns it.
- **Waybar/Polybar**: `muzeec-cli status --waybar` emits
  `{"text":"▶ Title — Artist","tooltip":"…","class":"playing","alt":"…"}`.
- Single instance guaranteed by the socket; stale-socket detection via connect + version
  handshake. Clean SIGTERM/SIGINT save-state.

---

## 11. Config

`$XDG_CONFIG_HOME/muzeec-cli/config.toml` (all keys optional, defaults shown):

```toml
[library]
music_dirs = ["~/Music"]
follow_symlinks = false
watch = true
scan_workers = 0            # 0 = auto

[playback]
device = "default"
engine_sample_rate = 48000  # "auto" follows device
buffer_ms = 20
gapless = true
crossfade_secs = 0
crossfade_on_album_change_only = false
replaygain = "track"        # off | track | album
volume = 70
resume_on_start = true
idle_stop = false           # stop daemon when stopped and no clients (off = keep alive)

[ui]
theme = "catppuccin-mocha"
album_art = "auto"          # auto|kitty|sixel|iterm2|halfblocks|off
visualizer = "bars"         # bars|osc|off
fps = 30

[integrations]
mpris = true
notifications = true
notify_art = true

[eq]
enabled = false
preamp_db = 0.0
bands = [0,0,0,0,0,0,0,0,0,0]

[keys]
# "space" = "play-pause"
```

`muzeec-cli config edit` opens `$EDITOR`; `muzeec-cli config dump` prints effective config with
sources. Invalid values fail at startup with the file/line, never panic.

---

## 12. Packaging (Arch)

### PKGBUILD (sketch)

```bash
pkgname=muzeec-cli
pkgver=0.1.0
pkgrel=1
pkgdesc="Terminal music player: gapless, crossfade, library, MPRIS"
arch=('x86_64' 'aarch64')
url="https://github.com/<you>/muzeec-cli"
license=('MIT')
depends=('glibc' 'gcc-libs' 'alsa-lib' 'sqlite' 'dbus')
makedepends=('cargo' 'rust' 'pkgconf')
optdepends=('playerctl: MPRIS CLI control'
            'libnotify: desktop notifications'
            'ttf-nerd-fonts-symbols: TUI icons')
source=("muzeec-cli-$pkgver.tar.gz")
sha256sums=('SKIP')   # local build; real sums for AUR release

build() {
  cd "$pkgname-$pkgver"
  export CARGO_HOME="$srcdir/cargo-home"
  cargo fetch --locked
  cargo build --release --frozen --locked
  ./target/release/muzeec-cli completions bash > muzeec-cli.bash
  ./target/release/muzeec-cli completions zsh  > _muzeec-cli
  ./target/release/muzeec-cli completions fish > muzeec-cli.fish
  ./target/release/muzeec-cli man > muzeec-cli.1
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 target/release/muzeec-cli  "$pkgdir/usr/bin/muzeec-cli"
  install -Dm755 target/release/muzeec-cli-daemon "$pkgdir/usr/bin/muzeec-cli-daemon"
  install -Dm644 muzeec-cli.bash "$pkgdir/usr/share/bash-completion/completions/muzeec-cli"
  install -Dm644 _muzeec-cli     "$pkgdir/usr/share/zsh/site-functions/_muzeec-cli"
  install -Dm644 muzeec-cli.fish "$pkgdir/usr/share/fish/vendor_completions.d/muzeec-cli.fish"
  install -Dm644 muzeec-cli.1    "$pkgdir/usr/share/man/man1/muzeec-cli.1"
  install -Dm644 assets/muzeec-cli.desktop "$pkgdir/usr/share/applications/muzeec-cli.desktop"
  install -Dm644 assets/muzeec-cli.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/muzeec-cli.svg"
  install -Dm644 assets/muzeec-cli-daemon.service "$pkgdir/usr/lib/systemd/user/muzeec-cli-daemon.service"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
```

### Build flow (`scripts/build-pkg.sh`)

1. Guard: never run as root; never call `pacman -U`.
2. `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace` (skip with
   `--fast`).
3. Read version from `Cargo.toml`; verify it matches `PKGBUILD` (fail on drift).
4. Create a clean source tarball (exclude `target/`, `dist/`, `vendor/`, `.git/`) into
   `packaging/arch/`.
5. `cd packaging/arch && makepkg -sf --noconfirm --cleanbuild`.
6. Copy `muzeec-cli-$pkgver-$pkgrel-$CARCH.pkg.tar.zst` to `dist/`, print size + sha256 and
   exactly the install command for the user:

```
sudo pacman -U dist/muzeec-cli-0.1.0-1-x86_64.pkg.tar.zst
```

Optional flags: `--native` (`RUSTFLAGS="-C target-cpu=native"`, machine-local),
`--vendor` (hermetic `cargo vendor` build for offline/AUR).

Package checks after build: `pacman -Qlp` file list sane, `desktop-file-validate` passes,
`namcap dist/*.pkg.tar.zst` clean (or justified warnings), binary runs `--version` from a
temp prefix. The script skips validation tools that are not installed and warns instead of
trying to install packages.

> The tooling stops at the artifact. Installing is the user's `sudo pacman -U`.

---

## 13. Milestones

Each phase ends green (`fmt`, `clippy -D warnings`, tests) and is demoable.

### Phase 0 — Spikes & decisions (~0.5–1 day)
- [ ] Verify symphonia 0.6 + symphonia-opus decode MP3/FLAC/Vorbis/Opus/AAC/ALAC/M4A.
- [ ] Prove cpal opens the real device at 48 kHz and survives 10 min playback; compare
      ALSA vs pipewire feature on this machine.
- [ ] Verify FTS5 in Arch SQLite; `mpris-server`/zbus version compatibility; ratatui-image
      protocol works in the user's terminal.
- [ ] Write `docs/ARCHITECTURE.md` decisions log.
- **Gate:** a throwaway binary plays two files back-to-back with zero added silence, and
  crossfades two tones without clicks.

### Phase 1 — Skeleton & data (~2 days)
- [ ] Workspace, CI-able scripts, error/logging/config/XDG paths.
- [ ] SQLite schema + migrations + FTS5 + queries; queue/playlist models with unit tests.
- [ ] Scanner (tags, art extraction, incremental, progress events) + fixtures.
- [ ] CLI skeleton (`clap`, JSON output, completions/man generation).
- **Gate:** scan 5k tracks < 30 s cold; re-scan unchanged < 1 s; search < 20 ms.

### Phase 2 — Engine (~4–6 days)
- [ ] Output abstraction + cpal sink + resampler + rings + realtime mixer.
- [ ] Decode/gapless/seek/volume/fades/ReplayGain; loop modes.
- [ ] Crossfade; auto-advance; device hot-swap; error recovery.
- [ ] Headless test suite: gapless length/continuity, crossfade envelope, seek accuracy,
      no-clip limiter, 3-strike skip.
- **Gate:** sample-level gapless test passes for FLAC/MP3/Opus; 2-hour soak with no
  underruns, drift, or leaks.

### Phase 3 — Daemon & IPC (~2 days)
- [ ] Single-instance daemon, socket protocol, auto-spawn, state persistence/resume.
- [ ] Control thread + snapshots; event stream; reconnect handling.
- [ ] CLI commands wired end-to-end.
- **Gate:** two clients share one player; `muzeec-cli play`, `status --json`, `queue` round-trip;
  kill -9 daemon → clean recovery.

### Phase 4 — Desktop integration (~1–2 days)
- [ ] MPRIS2 full surface; playerctl/KDE validation.
- [ ] Notifications + art hint; `.desktop`, icon, systemd unit.
- [ ] `status --waybar`, `doctor`.
- **Gate:** `playerctl play-pause/next/metadata` works; media keys work in the desktop;
  notification shows art.

### Phase 5 — TUI (~4–6 days)
- [ ] Layout, panes, navigation, selection, multi-select, sorting, command bar, help.
- [ ] Search overlay (nucleo), queue/history/playlist editing, ratings.
- [ ] Album art rendering, visualizer, progress/seek, volume, theme engine, mouse.
- [ ] Snapshot tests (ratatui TestBackend), keybinding tests.
- **Gate:** a full listening session (browse → queue → edit → save playlist → resume) needs
  no CLI; 60 fps interaction feel, < 5% idle CPU.

### Phase 6 — Playlists & mix (~2 days)
- [ ] M3U8 import/export, dedupe/missing-file report.
- [ ] Mix scoring + endless autoqueue + smart shuffle; avoid-repeat window.
- **Gate:** mixed queue is genre/BPM coherent in a blind listen test; no repeats inside
  the configured window.

### Phase 7 — Polish & hardening (~2–3 days)
- [ ] EQ (config-only), keyboard/mouse edge cases, huge-library performance (100k tracks),
      corrupt-file fuzzing, error messages, man page/README/docs.
- [ ] Accessibility: 16-color themes, `NO_COLOR`, non-UTF8 filenames.
- **Gate:** 100k-track library browse/search stays responsive; `muzeec-cli doctor` all green.

### Phase 8 — Package & handoff (~0.5 day)
- [ ] Final PKGBUILD, icon, desktop file, service unit, build script.
- [ ] `makepkg` → `dist/muzeec-cli-<ver>-1-x86_64.pkg.tar.zst`; namcap/desktop validation.
- [ ] Print `sudo pacman -U …`; **do not install**.
- **Gate:** artifact installs on a clean Arch/CachyOS user as expected (user-run), binary,
  MPRIS, completions, man page, icon all present.

Estimated total: **~3–4 weeks part-time** (approximately 18–25 focused days).

---

## 14. Testing strategy

| Level | What |
|---|---|
| Unit | queue/playlist ops, loop/shuffle invariants, search parsing/ranking, config, path rules, mix scoring determinism |
| Engine | `NullSink`/`FileSink` tests for gapless sample continuity, crossfade equal-power curve, seek frame accuracy, limiter ceiling, resampler length, gapless trim on MP3/AAC/Opus fixtures |
| DB | migration from empty + v1→v2, FTS5 sync triggers, concurrent reader/writer, 100k-row query benchmarks (criterion) |
| IPC | protocol round-trip, malformed/oversized messages, version mismatch, reconnect |
| TUI | `TestBackend` snapshots (insta) for layout/help/search; keymap coverage |
| Integration | daemon + fake sink: play, pause, seek, next, loop, playlist save/restore, MPRIS session via `dbus-run-session` |
| Manual | real device soak (2 h), media keys on KDE, kitty/sixel/foot/tmux art, PipeWire device switch |
| Fuzz | tag parser paths (`cargo-fuzz`: lofty wrapper), IPC JSON, M3U parser |

Fixtures: tiny CC0 sine/noise files (WAV/FLAC/MP3/Opus/ALAC/M4A) plus tag/art variants,
generated by `scripts/gen-fixtures.sh` (ffmpeg locally, committed outputs, < 2 MB total).

Performance budgets: startup < 100 ms warm; search < 20 ms/50k; scan < 30 s/5k tracks;
memory < 150 MB with art cache capped; no allocation in the audio callback; 0 underruns in
a 2-hour soak.

---

## 15. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Gapless trim wrong for some MP3/AAC | Phase 0 proves per-codec trims; golden tests with known encoders; fallback to duration-based trim + documented caveat |
| cpal/ALSA device quirks (PipeWire bridge, buffer wakeups) | Engine rate fixed + resample; device retry/hot-swap; selectable pipewire/pulse features; real-hardware soak in Phase 2 |
| Realtime glitches in the callback | Preallocated lock-free design, no locks/alloc/IO, 3-buffer cushion, underrun counter in `doctor` |
| Opus/AAC coverage gaps | `symphonia-opus`; AAC via symphonia `aac`; ALAC via `alac`; worst case add libmpv feature backend |
| MPRIS crate churn/zbus version mismatch | Phase 0 pins compatible versions; fallback is a small direct zbus interface (~200 lines) |
| Terminal art unsupported (SSH/tmux) | Halfblocks fallback + `art=off`; `doctor` reports detected protocol |
| Packaging deps/offline build | `cargo fetch --locked` in prepare; `--vendor` hermetic option; verify on clean chroot if available |
| Scope creep | Non-goals list; EQ/lyrics/smart-playlists explicitly post-v1 |
| Huge libraries slow the TUI | SQL prefilter + nucleo top-N; virtualized lists only render the viewport |

---

## 16. Definition of done (v1.0)

- [ ] Gapless verified at sample level (FLAC, MP3, Opus) with no audible/measurable gap.
- [ ] Crossfade 0–12 s, equal power, click-free; loop off/one/all; shuffle.
- [ ] Queue (add/play-next/move/remove/clear/shuffle), history, resume after restart.
- [ ] Playlists editable in TUI and CLI, M3U8 import/export.
- [ ] Library explorer + incremental scan + watch; FTS search with filters; mix/auto-DJ.
- [ ] Album art in TUI (kitty/sixel/iterm2/halfblocks) and MPRIS artUrl.
- [ ] MPRIS2 validated with `playerctl` and desktop media keys; notifications; `.desktop`;
      icon; systemd unit shipped disabled.
- [ ] CLI parity for all player actions, `--json`, Waybar mode, `doctor`, completions, man.
- [ ] ReplayGain (tags + analyze) and config-only EQ.
- [ ] Tests green, clippy clean, 100k-track performance targets met.
- [ ] `dist/muzeec-cli-<ver>-1-x86_64.pkg.tar.zst` built; install command handed to the user
      (`sudo pacman -U …`) — **nothing installed by the tooling**.

---

## 17. Open decisions (defaults chosen, override any)

1. **Name** — `muzeec-cli` binaries (`muzeec-cli`, `muzeec-cli-daemon`), config dir `~/.config/muzeec-cli`.
2. **Audio stack** — pure Rust symphonia+cpal (recommended); libmpv only as fallback.
3. **Daemon model** — always daemon-backed (enables MPRIS, resume, multi-client).
4. **Output backend default** — decided in Phase 0 from real-device tests (ALSA vs pipewire).
5. **EQ** — post-v1 unless you want it in the first package.
6. **License** — MIT.
```
