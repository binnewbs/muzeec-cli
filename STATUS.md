# Implementation status

Tracking what exists in the tree versus `PLAN.md`. Verified with
`cargo test --workspace` (51 tests), `cargo clippy --workspace --all-targets
-- -D warnings`, and an end-to-end daemon/CLI smoke test (scan → play → gapless
advance → seek → queue → playlist → art).

## Done

- **Workspace**: four crates, pinned deps, committed `Cargo.lock`, rustfmt,
  clippy, justfile, MIT license.
- **Config & paths**: TOML config with validation, XDG dirs, `MUZEEC_CLI_ROOT`
  test isolation, tilde expansion.
- **Database**: SQLite (WAL, FTS5, triggers), migrations, tracks/albums/
  playlists/queue/history/ratings, incremental upserts, derived albums,
  search ranking with BM25 field boosts and filters.
- **Library**: rayon tag scanning via `lofty` (MP3/FLAC/Ogg/Opus/M4A/WAV/…),
  filename fallbacks, incremental `(size, mtime)` skip, stale pruning, art
  extraction (embedded + sidecar) with BLAKE3 cache/thumbnails/LRU, debounced
  `notify` watcher.
- **Engine**: Symphonia decode (+ pure-Rust Opus decoder registered), streaming
  resampler, lock-free SPSC ring, real-time callback with perceptual volume,
  ReplayGain gain + true-peak clamp, soft limiter, optional 10-band EQ, gapless
  handoff via preloaded next track, equal-power crossfade path, seek, pause,
  stop, device fallback to a silent sink, snapshots via `arc-swap`.
- **Daemon**: single instance, 0600 Unix socket with `SO_PEERCRED` uid check,
  NDJSON IPC with `hello`/`subscribe`, full command router (player, queue,
  playlists, library, search, mix, art, doctor), state persistence/resume,
  SIGINT/SIGTERM save, MPRIS2 server + notifications.
- **CLI**: every command in the plan's surface, daemon auto-spawn, `--json`,
  `--waybar`, `--short`, completions (bash/zsh/fish), man page, config
  path/edit/dump, `doctor`, M3U import/export, ANSI art render.
- **TUI**: sidebar (artists/albums/tracks/genres/playlists/queue), track list,
  now-playing panel, progress gauge, header/footer, search and command bar,
  playback/queue/seek/volume keys, periodic refresh.
- **Packaging/assets**: PKGBUILD, `scripts/build-pkg.sh` (never installs),
  fixtures script, dev-daemon script, SVG icon, `.desktop`, systemd user unit,
  docs (README, ARCHITECTURE, CONFIG, KEYBINDS, CONTRIBUTING).
  The script produced `dist/muzeec-cli-0.1.0-1-x86_64.pkg.tar.zst` (6.3 MB);
  contents verified with `pacman -Qlp`, binaries run from an extracted prefix,
  and the install command is printed without installing anything.

### Packaging notes

- `build-pkg.sh` passes `--nodeps` and omits `--syncdeps`: the toolchain may be
  from rustup rather than pacman's `rust`/`cargo` packages.
- `PKGBUILD` clears `LDFLAGS` and strips `-flto=auto` from `CFLAGS` for the
  Rust build. Arch's makepkg adds both; GCC LTO bitcode inside the bundled
  SQLite archive cannot be linked by lld, and `--as-needed` obscures the
  failure. `--remap-path-prefix` removes `$srcdir` references from binaries.


## Partial / follow-ups

- **TUI album art & spectrum**: `art --render` and MPRIS `artUrl` work; the
  in-TUI ratatui-image widget and FFT visualizer are not wired yet.
- **TUI fuzzy search**: SQL prefilter is used; nucleo reranking is not.
- **Daemon resume**: restores queue/volume and loads paused at the saved
  position; the exact saved position is approximate on first restart.
- **Crossfade**: implemented in the worker (equal-power mixing, tested math);
  no fixture-based envelope test and not exercised via config in the smoke run.
- **MPRIS**: implemented against `mpris-server` 0.11 and compiles; runtime
  validation under `dbus-run-session`/`playerctl` has not been performed here.
- **Mix**: scoring and weighted sampling implemented; endless auto-extend when
  three tracks remain is not yet automatic.
- **EQ**: DSP and config exist, off by default; no UI.
- **Performance gates** (100k tracks, 2-hour soak) not measured in this
  environment.

## Notes

- The streaming resampler is built-in rather than `rubato` (rubato 5's
  `audioadapter` API is a poor fit for the real-time streaming path). See
  `docs/ARCHITECTURE.md`.
- Engine track-boundary events fire within the ~250 ms decode look-ahead, which
  keeps audio gapless while bounding the reporting skew.
